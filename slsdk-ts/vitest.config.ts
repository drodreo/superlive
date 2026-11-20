import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // jsdom: the ui hooks (useShellState / useShellTheme) read window.__SL_SHELL__,
    // which only exists in a DOM-like global scope.
    environment: "jsdom",
  },
});
