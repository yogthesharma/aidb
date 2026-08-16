import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type ProxyOptions } from "vite";

const aidbServe = process.env.AIDB_SERVE_URL ?? "http://127.0.0.1:8080";

function spaHtmlBypass(req: { method?: string; headers: { accept?: string } }) {
  const accept = req.headers.accept ?? "";
  if (req.method === "GET" && accept.includes("text/html")) {
    return "/index.html";
  }
}

function injectBearer(proxyReq: {
  getHeader: (name: string) => unknown;
  setHeader: (name: string, value: string) => void;
}) {
  const token = process.env.AIDB_BEARER || process.env.AIDB_TOKEN;
  if (token && !proxyReq.getHeader("authorization")) {
    proxyReq.setHeader("Authorization", `Bearer ${token}`);
  }
}

const configure: NonNullable<ProxyOptions["configure"]> = (proxy) => {
  proxy.on("proxyReq", (proxyReq) => {
    injectBearer(proxyReq);
  });
  proxy.on("proxyReqWs", (proxyReq) => {
    injectBearer(proxyReq);
  });
};

const proxy: Record<string, ProxyOptions> = {
  "/sql": { target: aidbServe, changeOrigin: true, bypass: spaHtmlBypass, configure },
  "/health": { target: aidbServe, changeOrigin: true, configure },
  "/ws": { target: aidbServe, ws: true, changeOrigin: true, configure },
};

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5173,
    open: true,
    proxy,
  },
  preview: {
    host: "127.0.0.1",
    port: 4173,
    open: true,
    proxy,
  },
});
