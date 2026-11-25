import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: { conditions: ["browser"] },
  test: {
    include: ["tests/frontend/**/*.{test,spec}.{ts,tsx}"],
    environment: "jsdom",
    server: {
      deps: { inline: ["@m3e/react", "@lit/react"] },
    },
    setupFiles: ["./tests/frontend/test-setup.ts"],
  },
});