//! A setting: a field whose choices come from the backend, such as the
//! calendar new events go to. Offered once credentials are stored, outside
//! the credential form, and saved on its own as soon as it changes. The
//! panel draws it loading, and it loads its choices once the page is up.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::response::Response;
use koan_kit_web as kit;
use serde::Deserialize;

use super::stamp;
use crate::auth::extract::FragmentSession;
use crate::backend::choices::{self, Choice};
use crate::backend::sync::sync;
use crate::config::{CredentialField, Mcp};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{CredentialSet, TokenMeta};
use crate::web::{Draw, flash, one};

/// Where a setting loads its choices from, and where a change is saved.
pub(in crate::web) const SETTING: &str = "/dashboard/{mcp}/settings/{field}";

#[derive(Template)]
#[template(path = "dashboard/setting.html")]
pub(super) struct Setting {
    mcp: String,
    field: String,
    label: String,
    state: SettingState,
}

pub(super) enum SettingState {
    Loading,
    /// The choices, as `(value, label)`, the first the backend's own default.
    Ready {
        options: Vec<(String, String)>,
        selected: String,
    },
    /// The backend gave no choices: its reason, in the control's place.
    Failed(String),
}

impl Setting {
    pub(super) fn id_of(mcp: &str, field: &str) -> String {
        format!("mcp-{mcp}-setting-{field}")
    }

    /// The setting as the page draws it: its choices when the backend's
    /// answer stands, and loading otherwise.
    pub(super) fn of(
        s: &AppState,
        sub: &str,
        mcp: &Mcp,
        field: &CredentialField,
        stored: Option<(&CredentialSet, &TokenMeta)>,
    ) -> Self {
        let answer = s
            .questions
            .choices
            .peek(&key(sub, mcp, field, stored.map(|(_, meta)| meta)));
        let current = stored.and_then(|(values, _)| values.get(&field.id));
        Self::new(mcp, field, answer.map(|a| SettingState::of(a, current)))
    }

    fn new(mcp: &Mcp, field: &CredentialField, state: Option<SettingState>) -> Self {
        Self {
            mcp: mcp.id.clone(),
            field: field.id.clone(),
            label: field.label.clone(),
            state: state.unwrap_or(SettingState::Loading),
        }
    }

    fn id(&self) -> String {
        Self::id_of(&self.mcp, &self.field)
    }

    fn url(&self) -> String {
        SETTING
            .replace("{mcp}", &self.mcp)
            .replace("{field}", &self.field)
    }
}

impl SettingState {
    /// The usable choices, with the stored value chosen. A stored value the
    /// backend no longer offers chooses nothing rather than something else.
    fn of(answer: Result<Vec<Choice>, String>, current: Option<&String>) -> Self {
        let choices = match answer {
            Ok(choices) => choices,
            Err(reason) => return Self::Failed(reason),
        };
        let default = choices.iter().find(|c| c.is_default).map(Choice::label);
        let mut options = vec![(
            String::new(),
            match default {
                Some(label) => format!("account default ({label})"),
                None => "account default".into(),
            },
        )];
        let mut selected = String::new();
        for c in choices.iter().filter(|c| c.usable()) {
            if current.is_some_and(|v| *v == c.value || v == c.label()) {
                selected = c.value.clone();
            }
            let label = match c.is_default {
                true => format!("{} — account default", c.label()),
                false => c.label().to_string(),
            };
            options.push((c.value.clone(), label));
        }
        Self::Ready { options, selected }
    }
}

/// The question a setting asks: its choices, for these credentials as last
/// saved.
fn key(sub: &str, mcp: &Mcp, field: &CredentialField, stored: Option<&TokenMeta>) -> String {
    format!("{sub}\n{}\n{}\n{}", mcp.id, field.id, stamp(stored))
}

fn setting<'a>(mcp: &'a Mcp, field_id: &str) -> AppResult<(&'a CredentialField, &'a str)> {
    mcp.fields
        .iter()
        .find(|f| f.id == field_id)
        .and_then(|f| Some((f, f.options_query.as_deref()?)))
        .ok_or(AppError::NotFound)
}

/// The setting with its choices, asked of the backend through the server's
/// questions. A failure answers with the backend's reason in the control's
/// place, and the rest of the page is unaffected.
pub(in crate::web) async fn serve(
    State(s): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path((mcp_id, field_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let mcp = super::mcp(&s, &mcp_id)?;
    let (field, query) = setting(mcp, &field_id)?;
    let Some((values, meta)) = s
        .store
        .get_credentials_with_meta(&session.sub, &mcp.id, mcp.primary_field())
        .await?
    else {
        let failed = SettingState::Failed("Connect the account first".into());
        return Ok(one(Setting::new(mcp, field, Some(failed)).draw()));
    };
    let key = key(&session.sub, mcp, field, Some(&meta));
    let answer = {
        let (s, mcp, values, query) = (s.clone(), mcp.clone(), values.clone(), query.to_string());
        s.questions
            .choices
            .clone()
            .answer(key, async move {
                choices::fetch(&s, &mcp, &values, &query)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|| Err("the backend could not be asked".into()))
    };
    let state = SettingState::of(answer, values.get(&field.id));
    Ok(one(Setting::new(mcp, field, Some(state)).draw()))
}

#[derive(Deserialize)]
pub(in crate::web) struct Change {
    value: String,
}

/// Save one setting, then tell the backend. The select already shows the
/// choice, so the answer is a flash saying how it went.
pub(in crate::web) async fn save(
    State(s): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path((mcp_id, field_id)): Path<(String, String)>,
    Form(change): Form<Change>,
) -> AppResult<Response> {
    let mcp = super::mcp(&s, &mcp_id)?;
    let (field, _) = setting(mcp, &field_id)?;
    let value = change.value.trim();
    if value.chars().any(char::is_control) {
        let complaint = format!("{} contains characters that can't be sent", field.label);
        return Ok(askama_datastar::events([flash(Err(&complaint))]));
    }
    let saved = s
        .store
        .update_credentials(&session.sub, &mcp.id, mcp.primary_field(), |stored| {
            let mut values = stored.cloned().ok_or("Connect the account first")?;
            match value.is_empty() {
                true => values.remove(&field.id),
                false => values.insert(field.id.clone(), value.to_string()),
            };
            Ok::<_, &str>(values)
        })
        .await?;
    let said = match saved {
        Err(complaint) => Err(complaint.to_string()),
        Ok((stored, values)) => match sync(&s, mcp, &values, stored.as_ref()).await {
            Some(refused) => Err(refused),
            None => Ok(format!("{} saved", field.label)),
        },
    };
    Ok(askama_datastar::events([flash(
        said.as_deref().map_err(String::as_str),
    )]))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::{Request, header};

    use super::*;

    fn choice(value: &str, label: &str) -> Choice {
        Choice {
            value: value.into(),
            label: Some(label.into()),
            is_default: false,
            disabled: false,
            supports_events: true,
        }
    }

    fn ready(state: SettingState) -> (Vec<(String, String)>, String) {
        match state {
            SettingState::Ready { options, selected } => (options, selected),
            _ => panic!("not ready"),
        }
    }

    #[test]
    fn choices_are_filtered_labelled_and_chosen() {
        let choices = vec![
            Choice {
                is_default: true,
                ..choice("work", "Work")
            },
            Choice {
                disabled: true,
                ..choice("ro", "Read only")
            },
            Choice {
                supports_events: false,
                ..choice("tasks", "Tasks")
            },
            choice("home", "Home"),
        ];
        let (options, selected) = ready(SettingState::of(Ok(choices), Some(&"home".into())));
        let offered: Vec<&str> = options.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(
            offered,
            ["", "work", "home"],
            "read-only and task-only dropped"
        );
        assert_eq!(options[0].1, "account default (Work)");
        assert_eq!(options[1].1, "Work — account default");
        assert_eq!(selected, "home");
    }

    /// A stored value that no longer exists upstream must not silently choose
    /// something else.
    #[test]
    fn nothing_is_chosen_when_the_stored_value_is_gone() {
        let (_, selected) = ready(SettingState::of(
            Ok(vec![choice("home", "Home")]),
            Some(&"deleted".into()),
        ));
        assert_eq!(selected, "");
    }

    fn setting(state: SettingState) -> String {
        Setting {
            mcp: "caldav".into(),
            field: "calendar".into(),
            label: "Calendar".into(),
            state,
        }
        .render()
        .unwrap()
    }

    /// A patch aimed at an id nothing carries fails silently.
    #[test]
    fn every_state_carries_the_id_the_panel_places() {
        let id = format!(r#"id="{}""#, Setting::id_of("caldav", "calendar"));
        for state in [
            SettingState::Loading,
            SettingState::Failed("no".into()),
            SettingState::Ready {
                options: vec![],
                selected: String::new(),
            },
        ] {
            assert!(setting(state).contains(&id));
        }
    }

    #[test]
    fn only_a_loading_setting_loads() {
        let loading = setting(SettingState::Loading);
        assert!(loading.contains(r#"data-url="/dashboard/caldav/settings/calendar""#));
        assert!(!setting(SettingState::Failed("no".into())).contains("data-init"));
    }

    #[tokio::test]
    async fn a_change_deserializes_from_a_urlencoded_body() {
        for (body, expected) in [("value=home", "home"), ("value=", "")] {
            let request = Request::builder()
                .method("POST")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap();
            let Form(change) = Form::<Change>::from_request(request, &()).await.unwrap();
            assert_eq!(change.value, expected);
        }
    }
}
