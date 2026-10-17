/** 轻量 toast（模块级状态，任何组件都能直接推消息）。 */

import { ref } from "vue";

export type ToastKind = "ok" | "err" | "warn" | "info";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  message?: string;
  timeout: number;
}

const toasts = ref<Toast[]>([]);
let seed = 0;

export function pushToast(
  kind: ToastKind,
  title: string,
  message?: string,
  timeout = kind === "err" ? 9000 : 4500,
) {
  const id = ++seed;
  toasts.value = [...toasts.value, { id, kind, title, message, timeout }];
  if (timeout > 0) {
    window.setTimeout(() => dismissToast(id), timeout);
  }
  return id;
}

export function dismissToast(id: number) {
  toasts.value = toasts.value.filter((toast) => toast.id !== id);
}

export function useToasts() {
  return { toasts, pushToast, dismissToast };
}

/** 把异常统一转成可读信息。 */
export function errorText(error: unknown): { title: string; message: string } {
  if (error instanceof Error) {
    const hint = (error as { hint?: string }).hint;
    return {
      title: "操作失败",
      message: hint ? `${error.message}（${hint}）` : error.message,
    };
  }
  return { title: "操作失败", message: String(error) };
}
