import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "es2021" },
  // `npx vitest run`: unit tests under tests/, DOM from happy-dom; the
  // Tauri bridge is mocked per test (tests/setup.js stubs invoke)
  test: {
    environment: "happy-dom",
    include: ["tests/**/*.test.js"],
    setupFiles: ["tests/setup.js"],
    restoreMocks: true,
  },
});
