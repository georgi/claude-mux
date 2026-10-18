// REST calls the app makes. Cookie sessions must send `X-Cmux` on
// mutations (CSRF); it is harmless elsewhere, so every call sends it.

import type {
  AddRepoReq,
  AnswerReq,
  CreatePlanReq,
  Created,
  DevicesResp,
  ErrorBody,
  PairCompleteResp,
  PushSubscribeReq,
  RepoInfo,
  ReposResp,
  SettingsResp,
  StatusResp,
  TranscriptResp,
  VapidResp,
} from "./types";

export class HttpError extends Error {
  constructor(
    public status: number,
    public body: ErrorBody | null,
  ) {
    super(body?.message ?? `HTTP ${status}`);
  }
}

interface Init {
  body?: unknown;
  token?: string;
  /** `Idempotency-Key`: a retry with the same key acts once. */
  key?: string;
}

async function call(method: string, path: string, init: Init = {}): Promise<Response> {
  const headers: Record<string, string> = { "X-Cmux": "1" };
  if (init.body !== undefined) headers["Content-Type"] = "application/json";
  if (init.token) headers["Authorization"] = `Bearer ${init.token}`;
  if (init.key) headers["Idempotency-Key"] = init.key;
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

/** A fresh `Idempotency-Key`. */
export function newKey(): string {
  return crypto.randomUUID();
}

/**
 * A mutation that is safe to retry: one key for every attempt, and one retry
 * when the network (not the server) failed, so a lost response does not act twice.
 */
async function once(method: string, path: string, body?: unknown, key = newKey()): Promise<Response> {
  try {
    return await call(method, path, { body, key });
  } catch (e) {
    if (e instanceof HttpError) throw e;
    return await call(method, path, { body, key });
  }
}

async function json<T>(r: Promise<Response>): Promise<T> {
  return (await (await r).json()) as T;
}

const seg = encodeURIComponent;

/** The REST calls behind the views. Tests pass a fake with the same shape. */
export interface Api {
  answer(item: number, req: AnswerReq, key?: string): Promise<void>;
  message(agent: string, text: string, steer: boolean): Promise<void>;
  stopAgent(agent: string): Promise<void>;
  resumeAgent(agent: string): Promise<void>;
  plan(plan: string, action: "accept" | "pause" | "resume" | "cancel"): Promise<void>;
  createPlan(req: CreatePlanReq): Promise<Created>;
  transcript(agent: string, after?: string): Promise<TranscriptResp>;
  repos(): Promise<ReposResp>;
  addRepo(req: AddRepoReq): Promise<RepoInfo>;
  devices(): Promise<DevicesResp>;
  revokeDevice(id: string): Promise<void>;
  status(): Promise<StatusResp>;
  settings(): Promise<SettingsResp>;
  vapid(): Promise<VapidResp>;
  pushSubscribe(req: PushSubscribeReq): Promise<void>;
  pushUnsubscribe(endpoint: string): Promise<void>;
}

export const api: Api = {
  async answer(item, req, key) {
    await once("POST", `/v1/items/${item}/answer`, req, key);
  },
  async message(agent, text, steer) {
    await once("POST", `/v1/agents/${seg(agent)}/messages`, { text, steer });
  },
  async stopAgent(agent) {
    await once("POST", `/v1/agents/${seg(agent)}/stop`);
  },
  async resumeAgent(agent) {
    await once("POST", `/v1/agents/${seg(agent)}/resume`);
  },
  async plan(plan, action) {
    await once("POST", `/v1/plans/${seg(plan)}/${action}`);
  },
  createPlan: (req) => json(once("POST", "/v1/plans", req)),
  transcript: (agent, after) =>
    json(call("GET", `/v1/agents/${seg(agent)}/transcript${after ? `?after=${seg(after)}` : ""}`)),
  repos: () => json(call("GET", "/v1/repos")),
  addRepo: (req) => json(once("POST", "/v1/repos", req)),
  devices: () => json(call("GET", "/v1/devices")),
  async revokeDevice(id) {
    await once("DELETE", `/v1/devices/${seg(id)}`);
  },
  status: () => json(call("GET", "/v1/status")),
  settings: () => json(call("GET", "/v1/settings")),
  vapid: () => json(call("GET", "/v1/push/vapid")),
  async pushSubscribe(req) {
    await once("POST", "/v1/push/subscribe", req);
  },
  async pushUnsubscribe(endpoint) {
    await once("POST", "/v1/push/unsubscribe", { endpoint });
  },
};
