/** 极薄的 fetch 封装：统一错误形状、统一 JSON 处理。 */

import type { ApiErrorBody } from "./types";

export const API_BASE = import.meta.env.VITE_API_BASE ?? "/api";

export class ApiError extends Error {
  readonly status: number;
  readonly kind: string;
  readonly hint: string;

  constructor(message: string, status: number, kind = "unknown", hint = "") {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.kind = kind;
    this.hint = hint;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response;
  try {
    response = await fetch(`${API_BASE}${path}`, {
      headers: { "content-type": "application/json" },
      ...init,
    });
  } catch (error) {
    throw new ApiError(
      `无法连接后端（${API_BASE}）：${error instanceof Error ? error.message : String(error)}`,
      0,
      "offline",
      "确认 acgn-server 已启动",
    );
  }

  const text = await response.text();
  let payload: unknown = null;
  if (text) {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = text;
    }
  }

  if (!response.ok) {
    const body = (payload ?? {}) as ApiErrorBody;
    const message =
      body.error?.message ??
      (typeof payload === "string" ? payload.slice(0, 200) : `HTTP ${response.status}`);
    throw new ApiError(
      message,
      response.status,
      body.error?.kind ?? "unknown",
      body.error?.hint ?? "",
    );
  }

  return payload as T;
}

export const http = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "POST", body: JSON.stringify(body ?? {}) }),
  patch: <T>(path: string, body: unknown) =>
    request<T>(path, { method: "PATCH", body: JSON.stringify(body) }),
  delete: <T>(path: string) => request<T>(path, { method: "DELETE" }),
};
