// Thin fetch wrapper that normalizes the LongBox error envelope into a
// typed ApiError. All other api/*.ts modules go through this.

export class ApiError extends Error {
  status: number;
  code: string;
  details: unknown;

  constructor(status: number, code: string, message: string, details?: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

const BASE = '/api';

export async function apiFetch<T>(path: string, options: RequestInit = {}): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${BASE}${path}`, {
      ...options,
      headers: { 'Content-Type': 'application/json', ...options.headers }
    });
  } catch (e) {
    throw new ApiError(0, 'network', e instanceof Error ? e.message : String(e));
  }

  const text = await res.text();
  let body: unknown = null;
  if (text.length > 0) {
    try {
      body = JSON.parse(text);
    } catch {
      body = { raw: text };
    }
  }

  if (!res.ok) {
    const env = (body as { error?: { code?: string; message?: string; details?: unknown } } | null)?.error;
    const code = env?.code ?? 'unknown';
    const message = env?.message ?? res.statusText ?? 'request failed';
    throw new ApiError(res.status, code, message, env?.details);
  }
  return body as T;
}
