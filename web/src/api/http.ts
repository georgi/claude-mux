// REST calls the app makes. Cookie sessions must send `X-Cmux` on
// mutations (CSRF); it is harmless elsewhere, so every call sends it.

import type { ErrorBody, PairCompleteResp } from "./types";

export class HttpError extends Error {
  constructor(
    public status: number,
    public body: ErrorBody | null,
  ) {
    super(body?.message ?? `HTTP ${status}`);
  }
}

async function call(method: string, path: string, init: { body?: unknown; token?: string } = {}): Promise<Response> {
  const headers: Record<string, string> = { "X-Cmux": "1" };
  if (init.body !== undefined) headers["Content-Type"] = "application/json";
  if (init.token) headers["Authorization"] = `Bearer ${init.token}`;
  const r = await fetch(path, {
    method,
    headers,
    credentials: "same-origin",
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  });
  if (!r.ok) {
    const body = (await r.json().catch(() => null)) as ErrorBody | null;
    throw new HttpError(r.status, body);
  }
  return r;
}

/** Is there a session cookie the server accepts? */
export async function hasSession(): Promise<boolean> {
  try {
    await call("GET", "/v1/status");
    return true;
  } catch (e) {
    if (e instanceof HttpError && e.status === 401) return false;
    throw e;
  }
}

/** Spend a pairing code for a device token. */
export async function pairComplete(code: string, deviceName: string): Promise<PairCompleteResp> {
  const r = await call("POST", "/v1/pair/complete", { body: { code, device_name: deviceName } });
  return (await r.json()) as PairCompleteResp;
}

/** Trade a device token for the `HttpOnly` session cookie. The token is not kept. */
export async function startSession(token: string): Promise<void> {
  await call("POST", "/v1/session", { token });
}

export async function endSession(): Promise<void> {
  await call("DELETE", "/v1/session");
}
