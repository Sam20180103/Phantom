import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import path from "path";
import { defineConfig, type PluginOption, type UserConfig } from "vite";
import Inspect from "vite-plugin-inspect";

const host = process.env.TAURI_DEV_HOST;

// Bundle/treemap analysis is opt-in: `ANALYZE=true pnpm build` emits stats.html.
const analyze = process.env.ANALYZE === "true";

// Module-graph inspector is opt-in via `pnpm dev:inspect`; keeps plain
// `pnpm dev` from paying its transform-tracking overhead on every run.
const inspectGraph = process.env.INSPECT === "true";

const isTauri = Boolean(process.env.TAURI_ENV_PLATFORM);

// https://vite.dev/config/
export default defineConfig(async ({ mode }): Promise<UserConfig> => ({
  plugins: [
    babel({
      presets: [reactCompilerPreset({ target: "19" })],
    }),
    react(),
    tailwindcss(),
    // Module-graph inspector at /__inspect (who-imports-what, per-plugin
    // transforms). Opt-in via `pnpm dev:inspect`, never in a production build.
    ...(mode === "development" && inspectGraph
      ? [Inspect() as PluginOption]
      : []),
    ...(analyze
      ? [
          (await import("rollup-plugin-visualizer")).visualizer({
            filename: "stats.html",
            template: "treemap",
            gzipSize: true,
            brotliSize: true,
            open: true,
          }) as PluginOption,
        ]
      : []),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      // Phantom runs in a plain browser, not a Tauri webview: redirect the IPC
      // primitives at our WebSocket transport. Aliasing here rather than
      // rewriting call sites is what keeps the ~600 upstream files unmodified
      // and this fork rebaseable. Desktop Tauri builds keep the real SDK.
      ...(isTauri
        ? {}
        : {
            "@tauri-apps/api/core": path.resolve(
              __dirname,
              "./src/lib/transport/core.ts",
            ),
            "@tauri-apps/api/event": path.resolve(
              __dirname,
              "./src/lib/transport/event.ts",
            ),
            // Desktop-shell APIs. These must be aliased too, not just the IPC ones:
            // the real modules throw when `__TAURI_INTERNALS__` is absent, and Terax
            // calls `getCurrentWindow()` during render — an exception there makes
            // React unmount the whole tree and the page goes blank.
            "@tauri-apps/api/window": path.resolve(
              __dirname,
              "./src/lib/transport/window.ts",
            ),
            "@tauri-apps/api/webview": path.resolve(
              __dirname,
              "./src/lib/transport/webview.ts",
            ),
            "@tauri-apps/api/app": path.resolve(
              __dirname,
              "./src/lib/transport/app.ts",
            ),
            "@tauri-apps/api/path": path.resolve(
              __dirname,
              "./src/lib/transport/path.ts",
            ),
            // Separate entry point from window/webview, and it throws harder: the
            // real one reads `__TAURI_INTERNALS__.metadata` at call time.
            "@tauri-apps/api/webviewWindow": path.resolve(
              __dirname,
              "./src/lib/transport/webviewWindow.ts",
            ),
            // Settings/session persistence, backed by localStorage instead of files.
            "@tauri-apps/plugin-store": path.resolve(
              __dirname,
              "./src/lib/transport/store.ts",
            ),
            // Agent notifications — delivered through the service worker so they
            // survive the tab being backgrounded, which is the case that matters.
            "@tauri-apps/plugin-notification": path.resolve(
              __dirname,
              "./src/lib/transport/notification.ts",
            ),
          }),
      // Shim keeps the ~117 kB CJS protocol package out of the bundle.
      "vscode-languageserver-protocol": path.resolve(
        __dirname,
        "./src/modules/lsp/lib/protocolShim.ts",
      ),
    },
  },
  build: {
    target:
      process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome120" : "es2022",
    chunkSizeWarningLimit: 1500,
    rolldownOptions: {
      input: {
        main: path.resolve(__dirname, "index.html"),
        settings: path.resolve(__dirname, "settings.html"),
      },
      // Oxc drops `debugger` by default. These calls return undefined, so
      // marking them pure lets DCE strip them from production builds.
      treeshake: {
        manualPureFunctions: [
          "console.debug",
          "console.info",
          "console.trace",
        ],
      },
      output: {
        manualChunks(id: string) {
          // Vite's __vitePreload helper is a virtual module. Left to Rollup it
          // gets hoisted into whichever chunk it happens to land in (observed:
          // the 480kB streamdown chunk), and since every lazy importer pulls the
          // helper, that heavy chunk gets dragged into the eager startup graph.
          // Pin it to the always-eager react chunk so it costs nothing extra.
          if (id.includes("vite/preload-helper") || id.includes("/vite/dist/"))
            return "react";

          if (!id.includes("node_modules")) return null;

          // Ubiquitous styling utils used by `cn()` on nearly every eager
          // component. Left unassigned, Rollup absorbs them into whichever
          // feature chunk claims them first (observed: streamdown), dragging
          // that heavy chunk into the eager graph. Pin them to react (eager).
          if (
            id.includes("/clsx/") ||
            id.includes("/tailwind-merge/") ||
            id.includes("/class-variance-authority/")
          )
            return "react";

          // Each AI provider SDK in its own chunk so unused providers
          // don't bloat the initial load (lazy-imported in agent.ts).
          if (id.includes("@ai-sdk/anthropic")) return "ai-anthropic";
          if (id.includes("@ai-sdk/google")) return "ai-google";
          if (id.includes("@ai-sdk/openai-compatible"))
            return "ai-openai-compat";
          if (id.includes("@ai-sdk/openai")) return "ai-openai";
          if (id.includes("@ai-sdk/cerebras")) return "ai-cerebras";
          if (id.includes("@ai-sdk/groq")) return "ai-groq";
          if (id.includes("@ai-sdk/xai")) return "ai-xai";
          if (id.includes("@ai-sdk/")) return "ai-sdk-shared";

          if (id.includes("/xterm/") || id.includes("@xterm/")) return "xterm";
          // Lang packs and legacy modes are dynamically imported by
          // languageResolver; give each its own named chunk so they load on
          // demand instead of being glued into the codemirror core chunk.
          // (bundle audit, issue #551)
          {
            const m = id.match(/@codemirror\/lang-([\w-]+)/);
            if (m) return `cm-lang-${m[1]}`;
          }
          {
            const m = id.match(/@codemirror\/legacy-modes\/mode\/([\w-]+)/);
            if (m) return `cm-legacy-${m[1]}`;
          }
          if (id.includes("@replit/codemirror-lang-svelte"))
            return "cm-lang-svelte";
          if (
            id.includes("@codemirror/") ||
            id.includes("@uiw/codemirror") ||
            id.includes("@replit/codemirror")
          )
            return "codemirror";
          if (id.includes("/streamdown/") || id.includes("@streamdown/"))
            return "streamdown";
          if (
            id.includes("/react-dom/") ||
            id.includes("/react/") ||
            id.includes("/scheduler/")
          )
            return "react";
          if (id.includes("@radix-ui/") || id.includes("/radix-ui/"))
            return "radix";

          return null;
        },
      },
    },
  },
  define: { __TERAX_BENCH__: "false" },
  clearScreen: false,
  server: {
    port: mode === "integration" ? 15174 : 1420,
    strictPort: true,
    host: mode === "integration" ? "127.0.0.1" : host || false,
    proxy: mode === "integration" ? {
      "/ws": { target: "ws://127.0.0.1:19889", ws: true },
      "/files": { target: "http://127.0.0.1:19889" },
      "/blob": { target: "http://127.0.0.1:19889" },
    } : undefined,
    hmr: mode !== "integration" && host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**", ...(mode === "integration" ? ["**/target/**"] : [])],
    },
  },
}));
