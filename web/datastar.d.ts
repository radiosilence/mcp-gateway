// The part of Datastar's API this app uses: askama_datastar's copy, imported
// as `datastar`.

declare module "datastar" {
  export interface ActionContext {
    el: HTMLElement;
    evt?: Event;
  }

  export function action<Args extends unknown[]>(plugin: {
    name: string;
    apply(ctx: ActionContext, ...args: Args): unknown;
  }): void;

  /** Detail of the `datastar-fetch` event the bundle dispatches on `document`. */
  export interface FetchDetail {
    type: "started" | "finished" | "error" | "retrying" | "retries-failed";
    el: HTMLElement;
    argsRaw: Record<string, string>;
  }
}
