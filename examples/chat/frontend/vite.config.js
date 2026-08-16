import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const relayApi = process.env.AIDB_API_URL ?? "http://127.0.0.1:8092";

export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 5175,
    proxy: {
      "/api": { target: relayApi, changeOrigin: true },
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 4175,
    proxy: {
      "/api": { target: relayApi, changeOrigin: true },
    },
  },
});
