import { Loader2Icon } from "lucide-react";
import { REGEXP_ONLY_DIGITS_AND_CHARS } from "input-otp";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { InputOTP, InputOTPGroup, InputOTPSeparator, InputOTPSlot } from "@/components/ui/input-otp";
import { Label } from "@/components/ui/label";
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
  const [code, setCode] = useState((params.get("code") ?? "").toUpperCase().slice(0, 8));
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

  const pair = (ev: React.FormEvent) => {
    ev.preventDefault();
    void run(async () => {
      const r = await pairComplete(code.trim().toUpperCase(), name.trim());
      await startSession(r.token);
    });
  };

  const useToken = (ev: React.FormEvent) => {
    ev.preventDefault();
    void run(() => startSession(token.trim()));
  };

  return (
    <main className="flex min-h-svh items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-xl">cmux</CardTitle>
          <CardDescription>Pair this browser with the server.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          <form onSubmit={pair} className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <Label htmlFor="pair-code">Pairing code</Label>
              <InputOTP
                id="pair-code"
                maxLength={8}
                value={code}
                onChange={(v) => setCode(v.toUpperCase())}
                pattern={REGEXP_ONLY_DIGITS_AND_CHARS}
                autoComplete="one-time-code"
                autoCapitalize="characters"
                disabled={busy}
                aria-invalid={error ? true : undefined}
                autoFocus
              >
                <InputOTPGroup>
                  <InputOTPSlot index={0} />
                  <InputOTPSlot index={1} />
                  <InputOTPSlot index={2} />
                  <InputOTPSlot index={3} />
                </InputOTPGroup>
                <InputOTPSeparator />
                <InputOTPGroup>
                  <InputOTPSlot index={4} />
                  <InputOTPSlot index={5} />
                  <InputOTPSlot index={6} />
                  <InputOTPSlot index={7} />
                </InputOTPGroup>
              </InputOTP>
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="pair-name">Device name</Label>
              <Input id="pair-name" value={name} onChange={(e) => setName(e.target.value)} required />
            </div>
            <Button type="submit" disabled={busy || code.length < 8 || !name.trim()}>
              {busy && <Loader2Icon className="animate-spin" />}
              Pair
            </Button>
          </form>
          <details className="group">
            <summary className="cursor-pointer rounded-md text-sm text-muted-foreground outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
              Use a device token
            </summary>
            <form onSubmit={useToken} className="mt-3 flex flex-col gap-3">
              <Label htmlFor="pair-token">Token</Label>
              <Input
                id="pair-token"
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                autoComplete="off"
                required
              />
              <Button type="submit" variant="outline" disabled={busy || !token.trim()}>
                {busy && <Loader2Icon className="animate-spin" />}
                Sign in
              </Button>
            </form>
          </details>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
        </CardContent>
        <CardFooter className="text-xs text-muted-foreground">
          <p>
            Run <code>cmux pair</code> on a paired machine to get a code.
          </p>
        </CardFooter>
      </Card>
    </main>
  );
}
