/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 覆盖 API 前缀，默认 /api（开发时由 Vite 代理到后端） */
  readonly VITE_API_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
