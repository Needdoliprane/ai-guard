import { defineConfig } from "vite";
import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { frontendIdentity } from "./src/build-identity.ts";

const packageVersion: unknown = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")).version;
if (typeof packageVersion !== "string") throw new Error("Missing frontend package version");
const bundleIdentity = frontendIdentity(packageVersion, process.env.AI_GUARD_BUILD_ID, process.env.AI_GUARD_BUILD_REVISION, process.env.AI_GUARD_VERSION);

export default defineConfig(({ command }) => {
  const nonce = randomBytes(18).toString("base64");
  return {
    define: { __AI_GUARD_FRONTEND_IDENTITY__: JSON.stringify(bundleIdentity) },
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
