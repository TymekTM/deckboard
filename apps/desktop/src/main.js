import { createApp } from "vue";
// solid-only stylesheet: every icon in the editor renders with `fas fa-*`
// (brand glyphs never rendered here - the classes are hardcoded `fas`),
// and skipping all.min.css keeps the brands/regular webfonts (~1 MB
// including their svg fallbacks) out of the bundle. Tablets style their
// own icons and do not use this CSS.
import "@fortawesome/fontawesome-free/css/solid.min.css";
import "./style.css";
import App from "./App.vue";

createApp(App).mount("#app");
