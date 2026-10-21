import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: { conditions: ["browser"] },
  test: {
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    environment: "jsdom",
    server: {
      deps: { inline: ["@m3e/react", "@lit/react"] },
    },
    setupFiles: ["./src/test-setup.ts"],
  },
});