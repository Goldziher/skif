import AutoImport from "unplugin-auto-import/vite";
import Components from "unplugin-vue-components/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [AutoImport({ imports: ["vue"], dirs: ["src/composables"] }), Components()],
});
