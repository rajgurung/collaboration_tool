// The parts of the global `htmx` object (loaded from /static/vendor) that our scripts use.
declare const htmx: {
  ajax(verb: string, path: string, context: { target: string; swap: string }): Promise<void>;
  trigger(elt: Element, event: string): void;
};
