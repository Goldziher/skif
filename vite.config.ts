import { defineConfig } from "vite";

export default defineConfig({
  build: {
    ssr: "action/index.ts",
    outDir: "action",
    emptyOutDir: false,
    target: "node24",
    minify: false,
    rollupOptions: {
      output: { format: "es", entryFileNames: "index.js" },
    },
  },
  ssr: { noExternal: true, target: "node" },
});
