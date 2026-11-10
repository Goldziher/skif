import Components from "unplugin-vue-components/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [Components({ include: [/\.vue$/, /\.vue\?vue/, /\.md$/] })],
});
