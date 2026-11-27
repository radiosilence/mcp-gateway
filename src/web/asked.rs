//! Questions the dashboard asks a backend, owned by the server rather than by
//! the request that wants the answer. A question is asked once however many
//! requests want its answer at the same moment, and the answer is remembered
//! for a while, so reloading the page, or a second tab opening it, does not
//! ask the backend again. Only a definite answer is remembered: a failure
//! (a backend that timed out or could not say) is shared with the requests
//! waiting on it and then forgotten, so the next request asks again.
//!
//! Each replica keeps its own answers, so with N replicas a question may be
//! asked up to N times a minute.
//!
//! A key names everything the answer depends on, including when the
//! credentials were last saved, so a change to them is a new question on
//! every replica without anything being told to forget.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::backend::choices::Choice;
use crate::backend::verify::Verified;

/// The questions of each kind the dashboard asks.
pub struct Questions {
    /// Whether a backend accepts the stored credentials, for the badge.
    pub(super) verdicts: Arc<Asked<Verified>>,
    /// A setting's choices, or why the backend gave none.
    pub(super) choices: Arc<Asked<Result<Vec<Choice>, String>>>,
}

impl Default for Questions {
    fn default() -> Self {
        Self {
            verdicts: Arc::new(Asked::new(Duration::from_secs(60), |v| {
                *v != Verified::Unknown
            })),
            choices: Arc::new(Asked::new(Duration::from_secs(60), Result::is_ok)),
        }
    }
}

/// What is known of one question: in flight, or answered at a moment.
enum Known<V> {
    Asking(watch::Receiver<Option<V>>),
    Answered(Instant, V),
}

/// The questions of one kind, by key.
pub(super) struct Asked<V> {
    known: Mutex<HashMap<String, Known<V>>>,
    /// How long an answer stands before the question is asked again.
    keep: Duration,
    /// Whether an answer is definite, and so remembered.
    definite: fn(&V) -> bool,
}

impl<V: Clone + Send + Sync + 'static> Asked<V> {
    pub(super) fn new(keep: Duration, definite: fn(&V) -> bool) -> Self {
        Self {
            known: Mutex::default(),
            keep,
            definite,
        }
    }

    /// The standing answer to `key`, if there is one, without asking: a page
    /// draws it at once rather than loading it.
    pub(super) fn peek(&self, key: &str) -> Option<V> {
        match self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)?
        {
            Known::Answered(at, answer) if at.elapsed() < self.keep => Some(answer.clone()),
            _ => None,
        }
    }

    /// The answer to `key`: the standing one, the one in flight once it
    /// lands, or `work`'s, asked now. The work runs on its own task, so a
    /// request that gives up does not cancel it for the others waiting.
    /// `None` only when the work panicked, which the next request asks again.
    pub(super) async fn answer<F>(self: &Arc<Self>, key: String, work: F) -> Option<V>
    where
        F: Future<Output = V> + Send + 'static,
    {
        let mut waiting = {
            let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
            match known.get(&key) {
                Some(Known::Answered(at, answer)) if at.elapsed() < self.keep => {
                    return Some(answer.clone());
                }
                // A question whose task died (it panicked) is asked again.
                Some(Known::Asking(waiting)) if waiting.has_changed().is_ok() => waiting.clone(),
                _ => {
                    let (tell, waiting) = watch::channel(None);
                    known.insert(key.clone(), Known::Asking(waiting.clone()));
                    let asked = self.clone();
                    tokio::spawn(async move {
                        let answer = work.await;
                        asked.settle(key, answer.clone());
                        let _ = tell.send(Some(answer));
                    });
                    waiting
                }
            }
        };
        waiting.wait_for(Option::is_some).await.ok()?.clone()
    }

    fn settle(&self, key: String, answer: V) {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        known.retain(|_, k| match k {
            Known::Asking(_) => true,
            Known::Answered(at, _) => at.elapsed() < self.keep,
        });
        match (self.definite)(&answer) {
            true => known.insert(key, Known::Answered(Instant::now(), answer)),
            false => known.remove(&key),
        };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn a_question_in_flight_is_asked_once() {
        let asked = Arc::new(Asked::<u32>::new(Duration::from_secs(60), |_| true));
        let times = Arc::new(AtomicUsize::new(0));
        let work = || {
            let times = times.clone();
            async move {
                times.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                7
            }
        };
        let (a, b) = tokio::join!(
            asked.answer("k".into(), work()),
            asked.answer("k".into(), work())
        );
        assert_eq!((a, b), (Some(7), Some(7)));
        assert_eq!(asked.answer("k".into(), work()).await, Some(7));
        assert_eq!(times.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_lapsed_answer_is_asked_again() {
        let asked = Arc::new(Asked::<u32>::new(Duration::from_millis(10), |_| true));
        assert_eq!(asked.answer("k".into(), async { 1 }).await, Some(1));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(asked.answer("k".into(), async { 2 }).await, Some(2));
    }

    #[tokio::test]
    async fn a_question_whose_work_panicked_is_asked_again() {
        let asked = Arc::new(Asked::<u32>::new(Duration::from_secs(60), |_| true));
        let answer = asked
            .answer("k".into(), async { panic!("the backend client broke") })
            .await;
        assert_eq!(answer, None);
        assert_eq!(asked.answer("k".into(), async { 3 }).await, Some(3));
    }

    #[tokio::test]
    async fn a_failure_is_shared_while_asked_and_then_asked_again() {
        let asked = Arc::new(Asked::<u32>::new(Duration::from_secs(60), |v| *v != 0));
        let times = Arc::new(AtomicUsize::new(0));
        let work = |answer| {
            let times = times.clone();
            async move {
                times.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                answer
            }
        };
        let (a, b) = tokio::join!(
            asked.answer("k".into(), work(0)),
            asked.answer("k".into(), work(0))
        );
        assert_eq!((a, b), (Some(0), Some(0)));
        assert_eq!(times.load(Ordering::SeqCst), 1, "shared while in flight");
        assert_eq!(asked.peek("k"), None, "a failure is not drawn from memory");
        assert_eq!(asked.answer("k".into(), work(5)).await, Some(5));
        assert_eq!(asked.peek("k"), Some(5));
    }
}
