/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";

// The control plane serves the built files on the API's own listener, so in production the
// page and the API share an origin. The dev server stands in for that by passing the API's
// paths to a control plane running locally.
export default defineConfig(({ mode }) => {
  const controlPlane = loadEnv(mode, ".", "HEPH_")["HEPH_DASHBOARD_API"] ?? "http://localhost:4000";

  return {
    plugins: [react()],
    server: {
      proxy: {
        "/api": controlPlane,
        "/auth": controlPlane,
      },
    },
    test: {
      environment: "jsdom",
      globals: true,
      setupFiles: ["./src/test-setup.ts"],
    },
  };
});
