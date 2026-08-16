import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const harborApi = process.env.AIDB_API_URL ?? "http://127.0.0.1:8091";

export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 5174,
    proxy: {
      "/api": { target: harborApi, changeOrigin: true },
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 4174,
    proxy: {
      "/api": { target: harborApi, changeOrigin: true },
    },
  },
});
