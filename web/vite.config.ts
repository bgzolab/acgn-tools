import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// 开发时前端跑 Vite，/api 代理到本地 rust 后端；
// 生产时 pnpm build 产出 web/dist，直接由 acgn-server 托管（未来 Tauri 同理）。
export default defineConfig({
  plugins: [vue()],
  server: {
    host: true,
    port: 5173,
    proxy: {
      "/api": {
        target: process.env.VITE_API_PROXY ?? "http://127.0.0.1:8787",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 1200,
  },
});
