import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, errorMessage } from "./lib/api";
import type { AppInfo, AuthStatus, Workspace } from "./lib/types";
import { AuthPanel } from "./components/AuthPanel";
import { BacklogPanel } from "./components/BacklogPanel";
import { ConfigPanel } from "./components/ConfigPanel";
import { RunPanel } from "./components/RunPanel";
import { Alert, Badge, Spinner } from "./components/ui";

type StepId = "auth" | "workspace" | "backlog" | "run";

const STEPS: { id: StepId; label: string; hint: string }[] = [
  { id: "auth", label: "Sign in", hint: "Connect to GitHub" },
  { id: "workspace", label: "Workspace", hint: "Repository and board" },
  { id: "backlog", label: "Backlog", hint: "Load and validate" },
  { id: "run", label: "Import", hint: "Create the issues" },
];

export default function App() {
  const queryClient = useQueryClient();
  const [step, setStep] = useState<StepId>("auth");
  const [picked, setPicked] = useState(false);

  const appInfo = useQuery<AppInfo>({
    queryKey: ["appInfo"],
    queryFn: api.appInfo,
  });

  const auth = useQuery<AuthStatus>({
    queryKey: ["auth"],
    queryFn: api.authStatus,
  });

  const workspace = useQuery<Workspace>({
    queryKey: ["workspace"],
    queryFn: api.loadWorkspace,
  });

  const setAuth = (s: AuthStatus) => queryClient.setQueryData(["auth"], s);
  const setWorkspace = (w: Workspace) =>
    queryClient.setQueryData(["workspace"], w);

  const signedIn = auth.data?.signedIn ?? false;
  const report = workspace.data?.validation ?? null;
  const config = workspace.data?.config ?? null;

  const configValid = Boolean(
    config && config.repo.trim() !== "" && config.repo.includes("/"),
  );

  const blockReason = useMemo(() => {
    if (!signedIn) return "Sign in to GitHub first.";
    if (!configValid)
      return "Set a repository in the Workspace step. It must look like owner/name.";
    if (!report) return "Load a tasks.json backlog in the Backlog step.";
    if (!report.ok)
      return "The backlog has validation errors. Fix them, then reload.";
    return null;
  }, [signedIn, configValid, report]);

  // Land on the first step that still needs attention, once, after the initial
  // load — afterwards the user's choice wins.
  useEffect(() => {
    const a = auth.data;
    const w = workspace.data;
    if (picked || !a || !w) return;
    if (!a.signedIn) setStep("auth");
    else if (!configValid) setStep("workspace");
    else if (!report?.ok) setStep("backlog");
    else setStep("run");
    setPicked(true);
  }, [picked, auth.data, workspace.data, configValid, report]);

  const loadError = appInfo.error ?? auth.error ?? workspace.error;

  const done: Record<StepId, boolean> = {
    auth: signedIn,
    workspace: configValid,
    backlog: Boolean(report?.ok),
    run: false,
  };

  const current = STEPS.find((s) => s.id === step) ?? STEPS[0];

  return (
    <div className="flex h-full flex-col">
      <header className="flex shrink-0 items-center justify-between gap-4 border-b border-[var(--color-edge)] px-6 py-3.5">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-[var(--color-accent)] text-sm font-bold text-[var(--color-accent-ink)]">
            G
          </div>
          <div>
            <div className="text-sm font-semibold">GitHub Importer</div>
            <div className="text-xs text-[var(--color-ink-muted)]">
              Backlog to GitHub Issues and Projects
            </div>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {appInfo.data?.portable ? (
            <Badge tone="accent">Portable</Badge>
          ) : null}
          {appInfo.data ? <Badge>v{appInfo.data.version}</Badge> : null}
          {signedIn ? <Badge tone="good">{auth.data?.login}</Badge> : null}
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <nav className="w-60 shrink-0 space-y-1 overflow-y-auto border-r border-[var(--color-edge)] p-3">
          {STEPS.map((s, i) => {
            const active = s.id === step;
            return (
              <button
                key={s.id}
                type="button"
                onClick={() => setStep(s.id)}
                className={`flex w-full items-start gap-3 rounded-lg px-3 py-2.5 text-left transition-colors ${
                  active
                    ? "bg-[var(--color-surface-sunken)]"
                    : "hover:bg-[var(--color-surface-sunken)]"
                }`}
              >
                <span
                  className={`mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full border text-xs font-medium ${
                    done[s.id]
                      ? "border-transparent bg-[var(--color-accent)] text-[var(--color-accent-ink)]"
                      : active
                        ? "border-[var(--color-accent)] text-[var(--color-accent)]"
                        : "border-[var(--color-edge)] text-[var(--color-ink-muted)]"
                  }`}
                >
                  {done[s.id] ? "✓" : i + 1}
                </span>
                <span className="min-w-0">
                  <span
                    className={`block text-sm ${active ? "font-medium" : ""}`}
                  >
                    {s.label}
                  </span>
                  <span className="block text-xs text-[var(--color-ink-muted)]">
                    {s.hint}
                  </span>
                </span>
              </button>
            );
          })}
        </nav>

        <main className="min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto max-w-3xl px-6 py-6">
            {loadError ? (
              <div className="mb-5">
                <Alert tone="bad" title="Could not read the workspace">
                  {errorMessage(loadError)}
                </Alert>
              </div>
            ) : null}

            {/*
              React Query types `.data` as `T | undefined` and an `isSuccess`
              check does not narrow it, so the loaded branch is built from
              locals that TypeScript can narrow properly.
            */}
            {(() => {
              const info = appInfo.data;
              const status = auth.data;
              const ws = workspace.data;
              if (!info || !status || !ws) return <Spinner label="Loading…" />;

              return (
                <>
                  <div className="mb-5">
                    <h1 className="text-lg font-semibold">{current.label}</h1>
                    <p className="text-sm text-[var(--color-ink-muted)]">
                      {current.hint}
                    </p>
                  </div>

                  {step === "auth" ? (
                    <AuthPanel
                      status={status}
                      appInfo={info}
                      onChanged={setAuth}
                    />
                  ) : null}

                  {step === "workspace" ? (
                    <ConfigPanel
                      workspace={ws}
                      appInfo={info}
                      onChanged={setWorkspace}
                    />
                  ) : null}

                  {step === "backlog" ? (
                    <BacklogPanel workspace={ws} onChanged={setWorkspace} />
                  ) : null}

                  {step === "run" ? (
                    <RunPanel
                      workspace={ws}
                      signedIn={signedIn}
                      canRun={blockReason === null}
                      blockReason={blockReason}
                    />
                  ) : null}
                </>
              );
            })()}
          </div>
        </main>
      </div>
    </div>
  );
}
