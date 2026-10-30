import { defineConfig } from "vite";
import { randomBytes } from "node:crypto";

export default defineConfig(({ command }) => {
  const nonce = randomBytes(18).toString("base64");
  return {
    html: command === "serve" ? { cspNonce: nonce } : undefined,
    plugins:
      command === "serve"
        ? [
            {
              name: "nonce-developpement-local",
              transformIndexHtml(html) {
                return html
                  .replace(
                    "style-src 'self'",
                    `style-src 'self' 'nonce-${nonce}'`,
                  )
                  .replace(
                    "connect-src 'self'",
                    "connect-src 'self' ws://127.0.0.1:5173 ws://localhost:5173",
                  );
              },
            },
          ]
        : [],
    server: {
      strictPort: true,
      proxy: {
        "/api": {
          target: process.env.AGENT_SANDBOX_GATEWAY || "http://127.0.0.1:8787",
        },
        "/healthz": {
          target: process.env.AGENT_SANDBOX_GATEWAY || "http://127.0.0.1:8787",
        },
      },
    },
    build: { target: "es2022", sourcemap: false },
  };
});
