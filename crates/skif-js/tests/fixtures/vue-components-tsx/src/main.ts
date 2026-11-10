import { createApp } from "vue";
import App from "./App.vue";
import { Toolbar } from "./Toolbar";

createApp(App).component("Toolbar", Toolbar).mount("#app");
