import { defineConfig } from "vite";
import type { Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const lowerReactMarkdownObjectHasOwn = (): Plugin => ({
  name: "lower-react-markdown-object-has-own",
  enforce: "pre",
  transform(code, id) {
    if (!id.includes("/node_modules/react-markdown/")) return null;
    const transformed = code.replaceAll(
      "Object.hasOwn(",
      "Object.prototype.hasOwnProperty.call(",
    );
    return transformed === code ? null : { code: transformed, map: null };
  },
});

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [lowerReactMarkdownObjectHasOwn(), react(), tailwindcss()],

  build: {
    target: "safari15",
    cssTarget: "safari15",
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1431,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
