// Compatibility entrypoint. Page turning and custom reader actions share the
// serialization and display rules in reader-bindings.ts.
export {
  DEFAULT_PREVIOUS_PAGE_BINDING,
  DEFAULT_NEXT_PAGE_BINDING,
  bindingFromKeyboardEvent,
  bindingFromMouseEvent,
  keyboardEventMatchesBinding,
  mouseEventMatchesBinding,
  formatPageTurnBinding,
} from "./reader-bindings";
