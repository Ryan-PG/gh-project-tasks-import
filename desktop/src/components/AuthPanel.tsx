import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, onDeviceFlow } from "../lib/api";
import type { AppInfo, AuthStatus, DeviceCodeInfo } from "../lib/types";
import { Alert, Badge, Button, Card, Checkbox, Field, SectionTitle, Spinner, TextInput } from "./ui";

/**
 * Sign-in, with the device flow as the default path.
 *
 * The token never reaches this component: `authStartDeviceFlow` returns a user
 * code, and `authPollDeviceFlow` resolves once Rust holds the token.
 */
export function AuthPanel({
  status,
  appInfo,
  onChanged,
}: {
  status: AuthStatus;
  appInfo: AppInfo | null;
  onChanged: (s: AuthStatus) => void;
}) {
  const [clientId, setClientId] = useState(status.clientId);
  const [pat, setPat] = useState("");
  const [showPat, setShowPat] = useState(false);
  const [device, setDevice] = useState<DeviceCodeInfo | null>(null);
  const [secondsLeft, setSecondsLeft] = useState<number | null>(null);
  const [busy, setBusy] = useState<null | "device" | "pat" | "gh" | "remember" | "signout">(null);
  const [error, setError] = useState<string | null>(null);
  // Mirrors `device` so the polling promise can tell whether it was cancelled.
  const flowToken = useRef(0);

  // Keep the field in step when the status changes underneath us (a fresh load,
  // or a client id compiled into the build).
  useEffect(() => {
    setClientId((current) => (current === "" ? status.clientId : current));
  }, [status.clientId]);

  // Countdown for the pending code. GitHub's own event also carries a
  // remainder, but it only arrives once per poll interval.
  useEffect(() => {
    if (!device) return;
    const timer = window.setInterval(() => {
      setSecondsLeft((s) => (s === null || s <= 0 ? s : s - 1));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [device]);

  // Status pushed from the Rust poller.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void onDeviceFlow((e) => {
      if (typeof e.secondsRemaining === "number") setSecondsLeft(e.secondsRemaining);
      if (e.status === "approved") {
        setDevice(null);
        setSecondsLeft(null);
      }
      if (e.status === "expired") {
        setError("The code expired before it was approved. Start again to get a new one.");
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  async function startDeviceFlow() {
    setError(null);
    setBusy("device");
    try {
      // The client id is read from saved preferences in Rust, so a freshly
      // typed one has to land there first.
      let current = status;
      if (clientId.trim() !== status.clientId) {
        current = await api.setClientId(clientId.trim());
        onChanged(current);
      }
      const info = await api.startDeviceFlow();
      setDevice(info);
      setSecondsLeft(info.expiresIn);

      flowToken.current += 1;
      const mine = flowToken.current;
      try {
        const next = await api.pollDeviceFlow();
        if (flowToken.current === mine) {
          onChanged(next);
          setDevice(null);
        }
      } catch (e) {
        if (flowToken.current === mine) setError(errorMessage(e));
      } finally {
        if (flowToken.current === mine) setBusy(null);
      }
    } catch (e) {
      setError(errorMessage(e));
      setBusy(null);
    }
  }

  async function cancelDeviceFlow() {
    flowToken.current += 1;
    await api.cancelDeviceFlow().catch(() => undefined);
    setDevice(null);
    setSecondsLeft(null);
    setBusy(null);
  }

  async function signInWithPat() {
    setError(null);
    setBusy("pat");
    try {
      onChanged(await api.setPat(pat));
      setPat("");
      setShowPat(false);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function importFromGh() {
    setError(null);
    setBusy("gh");
    try {
      onChanged(await api.importFromGh());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function toggleRemember(next: boolean) {
    setError(null);
    setBusy("remember");
    try {
      onChanged(await api.setRemember(next));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function signOut() {
    setError(null);
    setBusy("signout");
    try {
      onChanged(await api.signOut());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const formatRemaining = (s: number) =>
    `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  if (status.signedIn) {
    return (
      <Card>
        <SectionTitle
          title="Signed in"
          subtitle={status.method ?? undefined}
          right={
            <Button variant="danger" onClick={signOut} disabled={busy === "signout"}>
              {busy === "signout" ? "Signing out…" : "Sign out"}
            </Button>
          }
        />
        <div className="flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-full bg-[var(--color-surface-sunken)] text-base font-semibold">
            {(status.login ?? "?").slice(0, 1).toUpperCase()}
          </div>
          <div>
            <div className="font-medium">{status.name ?? status.login}</div>
            <div className="mono text-xs text-[var(--color-ink-muted)]">{status.login}</div>
          </div>
        </div>

        {status.scopes.length > 0 ? (
          <div className="mt-4">
            <div className="mb-1.5 text-xs font-medium text-[var(--color-ink-muted)]">
              Granted scopes
            </div>
            <div className="flex flex-wrap gap-1.5">
              {status.scopes.map((s) => (
                <Badge
                  key={s}
                  tone={status.missingScopes.includes(s) ? "warn" : "neutral"}
                >
                  {s}
                </Badge>
              ))}
            </div>
          </div>
        ) : null}

        {status.missingScopes.length > 0 ? (
          <div className="mt-4">
            <Alert tone="warn" title="Some scopes are missing">
              This token lacks{" "}
              <span className="mono">{status.missingScopes.join(", ")}</span>. Creating labels
              needs <span className="mono">repo</span>; adding issues to a project board needs{" "}
              <span className="mono">project</span>.
            </Alert>
          </div>
        ) : null}

        <div className="mt-5 border-t border-[var(--color-edge)] pt-4">
          <Checkbox
            checked={status.rememberToken}
            disabled={busy === "remember"}
            onChange={toggleRemember}
            label="Remember this token"
            hint="Off means the token lives in memory for this run only and nothing is written to disk."
          />
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <SectionTitle
        title="Sign in to GitHub"
        subtitle="The token stays inside the app and is never exposed to the interface."
      />

      {error ? (
        <div className="mb-4">
          <Alert tone="bad">{error}</Alert>
        </div>
      ) : null}

      {status.vaultError ? (
        <div className="mb-4">
          <Alert tone="warn" title="Stored token unavailable">
            {status.vaultError}
          </Alert>
        </div>
      ) : null}

      {device ? (
        <div className="space-y-4">
          <Alert tone="info" title="Enter this code on GitHub">
            <div className="mt-2 flex items-center gap-3">
              <span className="mono rounded-lg border border-[var(--color-edge)] bg-[var(--color-surface-raised)] px-3 py-1.5 text-lg font-semibold tracking-widest">
                {device.userCode}
              </span>
              <Button
                onClick={() => void navigator.clipboard.writeText(device.userCode)}
                variant="ghost"
              >
                Copy
              </Button>
            </div>
          </Alert>
          <div className="flex flex-wrap items-center gap-3">
            <Button variant="primary" onClick={() => void openUrl(device.verificationUri)}>
              Open {device.verificationUri.replace(/^https?:\/\//, "")}
            </Button>
            <Spinner label="Waiting for approval…" />
            {secondsLeft !== null ? (
              <span className="mono text-xs text-[var(--color-ink-muted)]">
                expires in {formatRemaining(secondsLeft)}
              </span>
            ) : null}
            <Button variant="ghost" onClick={cancelDeviceFlow}>
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-5">
          <div className="space-y-3">
            <Field
              label="OAuth App client id"
              hint="Register an OAuth App with device flow enabled, then paste its client id here. See the README for the one-time setup."
            >
              <TextInput
                value={clientId}
                onChange={setClientId}
                placeholder="Iv1.0123456789abcdef"
                spellCheck={false}
              />
            </Field>
            <Button
              variant="primary"
              onClick={startDeviceFlow}
              disabled={busy !== null || clientId.trim() === ""}
            >
              {busy === "device" ? "Starting…" : "Sign in with GitHub"}
            </Button>
            {appInfo?.builtinClientId ? (
              <p className="text-xs text-[var(--color-ink-muted)]">
                This build has a client id compiled in; it is used unless you override it above.
              </p>
            ) : null}
          </div>

          <div className="border-t border-[var(--color-edge)] pt-4">
            <button
              type="button"
              onClick={() => setShowPat((v) => !v)}
              className="text-sm font-medium text-[var(--color-accent)] hover:underline"
            >
              {showPat ? "Hide" : "Use a personal access token instead"}
            </button>
            {showPat ? (
              <div className="mt-3 space-y-3">
                <Field
                  label="Personal access token"
                  hint="A classic token needs the repo, project, and read:org scopes. A fine-grained token needs Issues, Contents, and Projects permissions."
                >
                  <TextInput
                    value={pat}
                    onChange={setPat}
                    type="password"
                    placeholder="github_pat_…"
                    spellCheck={false}
                  />
                </Field>
                <Button
                  variant="primary"
                  onClick={signInWithPat}
                  disabled={busy !== null || pat.trim() === ""}
                >
                  {busy === "pat" ? "Checking…" : "Sign in with token"}
                </Button>
              </div>
            ) : null}
          </div>

          {appInfo?.ghCliAvailable ? (
            <div className="border-t border-[var(--color-edge)] pt-4">
              <div className="mb-2 text-sm text-[var(--color-ink-muted)]">
                The <span className="mono">gh</span> CLI is installed and signed in on this
                machine.
              </div>
              <Button onClick={importFromGh} disabled={busy !== null}>
                {busy === "gh" ? "Reading…" : "Use my gh CLI session"}
              </Button>
            </div>
          ) : null}
        </div>
      )}
    </Card>
  );
}
