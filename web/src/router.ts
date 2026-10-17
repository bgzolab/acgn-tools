import { createRouter, createWebHistory } from "vue-router";

import DashboardView from "./views/DashboardView.vue";
import ImportView from "./views/ImportView.vue";
import LibraryView from "./views/LibraryView.vue";
import SettingsView from "./views/SettingsView.vue";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", name: "dashboard", component: DashboardView, meta: { title: "看板" } },
    { path: "/library", name: "library", component: LibraryView, meta: { title: "游戏库" } },
    { path: "/import", name: "import", component: ImportView, meta: { title: "导入与同步" } },
    { path: "/settings", name: "settings", component: SettingsView, meta: { title: "设置" } },
  ],
});
