import { useState } from "preact/hooks";
import { HttpError, pairComplete, startSession } from "../api/http";

interface Props {
  onPaired: () => void;
}

function defaultName(): string {
  const ua = navigator.userAgent;
  const os = /iPhone|iPad/.test(ua) ? "iOS" : /Android/.test(ua) ? "Android" : /Mac/.test(ua) ? "Mac" : /Linux/.test(ua) ? "Linux" : /Windows/.test(ua) ? "Windows" : "browser";
  return `web (${os})`;
}

/**
 * Pair with a code (from `cmux pair` or its QR code, which opens
 * `/pair?code=`), or paste a device token. Either way the token is traded for
 * the session cookie at once and never stored by the page.
 */
export function Pairing({ onPaired }: Props) {
  const params = new URLSearchParams(location.search);
  const [code, setCode] = useState(params.get("code") ?? "");
  const [name, setName] = useState(defaultName());
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      history.replaceState(null, "", "/");
      onPaired();
    } catch (e) {
      setError(e instanceof HttpError || e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const pair = (ev: Event) => {
    ev.preventDefault();
    void run(async () => {
      const r = await pairComplete(code.trim().toUpperCase(), name.trim());
      await startSession(r.token);
    });
  };

  const useToken = (ev: Event) => {
    ev.preventDefault();
    void run(() => startSession(token.trim()));
  };

  return (
    <div class="pairing">
      <h1>cmux</h1>
      <p class="muted">Pair this browser with the server.</p>
      <form onSubmit={pair}>
        <label>
          Pairing code
          <input
            value={code}
            onInput={(e) => setCode(e.currentTarget.value)}
            placeholder="ABCD2345"
            autocomplete="one-time-code"
            autocapitalize="characters"
            spellcheck={false}
            required
          />
        </label>
        <label>
          Device name
          <input value={name} onInput={(e) => setName(e.currentTarget.value)} required />
        </label>
        <button type="submit" disabled={busy}>
          Pair
        </button>
      </form>
      <details>
        <summary>Use a device token</summary>
        <form onSubmit={useToken}>
          <label>
            Token
            <input
              type="password"
              value={token}
              onInput={(e) => setToken(e.currentTarget.value)}
              autocomplete="off"
              required
            />
          </label>
          <button type="submit" disabled={busy}>
            Sign in
          </button>
        </form>
      </details>
      {error && <p class="error">{error}</p>}
      <p class="muted small">
        Run <code>cmux pair</code> on a paired machine to get a code.
      </p>
    </div>
  );
}
