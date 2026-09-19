import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage, onImportProgress } from "../lib/api";
import {
  PHASE_LABELS,
  STATUS_LABELS,
  type ImportEvent,
  type ImportSummary,
  type Phase,
  type TaskIssueStatus,
  type TaskStatus,
  type Workspace,
} from "../lib/types";
import { Alert, Badge, Button, Card, Checkbox, SectionTitle, Spinner } from "./ui";

/** A single task line in the live run log. */
type TaskLine = {
  key: number;
  phase: Phase;
  id: string;
  status: TaskStatus;
  detail: string | null;
};

const STATUS_TONE: Record<TaskStatus, "neutral" | "good" | "warn" | "bad" | "accent"> = {
  created: "good",
  would_create: "accent",
  added: "good",
  would_add: "accent",
  skipped: "neutral",
  already_set: "neutral",
  failed: "bad",
};

/**
 * The run itself: options, live progress, and the results.
 *
 * The importer writes state after every step, so a run that is interrupted
 * resumes where it stopped rather than creating duplicates.
 */
export function RunPanel({
  workspace,
  signedIn,
  canRun,
  blockReason,
}: {
  workspace: Workspace;
  signedIn: boolean;
  canRun: boolean;
  blockReason: string | null;
}) {
  const [dryRun, setDryRun] = useState(false);
  const [skipRelationships, setSkipRelationships] = useState(false);
  const [skipProject, setSkipProject] = useState(false);

  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<Phase | null>(null);
  const [phaseMessage, setPhaseMessage] = useState("");
  const [progress, setProgress] = useState<{ index: number; total: number } | null>(null);
  const [lines, setLines] = useState<TaskLine[]>([]);
  const [log, setLog] = useState<string[]>([]);
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [statuses, setStatuses] = useState<TaskIssueStatus[]>([]);
  const [cliOutput, setCliOutput] = useState<string | null>(null);

  const counter = useRef(0);
  const logRef = useRef<HTMLDivElement | null>(null);

  // Adopt the environment's SKIP_RELATIONSHIPS, since the CLI would honour it.
  useEffect(() => {
    setSkipRelationships(workspace.settings.skipRelationships);
  }, [workspace.settings.skipRelationships]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    void onImportProgress((event: ImportEvent) => {
      switch (event.kind) {
        case "phase":
          setPhase(event.phase);
          setPhaseMessage(event.message);
          if (event.phase !== "issues") setProgress(null);
          break;
        case "task":
          setProgress({ index: event.index, total: event.total });
          setLines((prev) => [
            ...prev.slice(-499),
            {
              key: counter.current++,
              phase: event.phase,
              id: event.id,
              status: event.status,
              detail: event.detail,
            },
          ]);
          break;
        case "log":
          setLog((prev) => [...prev.slice(-499), event.message]);
          break;
        case "done":
          setSummary(event.summary);
          setPhase("finished");
          setRunning(false);
          break;
        case "failed":
          setFailure(event.message);
          setRunning(false);
          break;
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

  // Keep the newest log line in view.
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines, log]);

  // Pull the resulting statuses once, after a real run.
  useEffect(() => {
    if (!summary || summary.dryRun) return;
    void api
      .importStatuses()
      .then(setStatuses)
      .catch(() => undefined);
  }, [summary]);

  const counts = useMemo(() => {
    const by: Partial<Record<TaskStatus, number>> = {};
    for (const l of lines) by[l.status] = (by[l.status] ?? 0) + 1;
    return by;
  }, [lines]);

  const percent =
    progress && progress.total > 0
      ? Math.round((progress.index / progress.total) * 100)
      : phase === "finished"
        ? 100
        : 0;

  async function start() {
    setLines([]);
    setLog([]);
    setSummary(null);
    setFailure(null);
    setStatuses([]);
    setCliOutput(null);
    setProgress(null);
    setPhase(null);
    setRunning(true);
    try {
      await api.startImport({ dryRun, skipRelationships, skipProject });
    } catch (e) {
      setFailure(errorMessage(e));
      setRunning(false);
    }
  }

  async function stop() {
    await api.cancelImport().catch(() => undefined);
    setLog((prev) => [...prev, "Stopping after the current step…"]);
  }

  async function showCliOutput() {
    try {
      const report = await api.dryRunPreview();
      setCliOutput(report.cliOutput);
    } catch (e) {
      setCliOutput(errorMessage(e));
    }
  }

  return (
    <div className="space-y-5">
      <Card>
        <SectionTitle
          title="Import"
          subtitle="Creates the issues, labels them, adds them to the board, then links the relationships."
        />

        {!canRun && blockReason ? (
          <div className="mb-4">
            <Alert tone="warn" title="Not ready to run">
              {blockReason}
            </Alert>
          </div>
        ) : null}

        {workspace.settings.skipRelationships ? (
          <div className="mb-4">
            <Alert tone="warn" title="Relationships are switched off by the environment">
              <span className="mono">SKIP_RELATIONSHIPS</span> is set to true, which switches the
              checkbox below on. Unset it and reload to import dependencies.
            </Alert>
          </div>
        ) : null}

        <div className="space-y-3">
          <Checkbox
            checked={dryRun}
            onChange={setDryRun}
            disabled={running}
            label="Dry run"
            hint="Reports what would happen without creating or changing anything on GitHub."
          />
          <Checkbox
            checked={skipProject}
            onChange={setSkipProject}
            disabled={running || !workspace.config?.project}
            label="Skip the project board"
            hint={
              workspace.config?.project
                ? `Leaves issues out of “${workspace.config.project}”.`
                : "No project is configured."
            }
          />
          <Checkbox
            checked={skipRelationships}
            onChange={setSkipRelationships}
            disabled={running}
            label="Skip parent links and dependencies"
            hint="Creates the issues but does not link them to each other."
          />
        </div>

        <div className="mt-5 flex flex-wrap items-center gap-3">
          <Button
            variant="primary"
            onClick={start}
            disabled={running || !canRun}
            title={!canRun ? (blockReason ?? undefined) : undefined}
          >
            {running ? "Running…" : dryRun ? "Run dry run" : "Start import"}
          </Button>
          {running ? (
            <>
              <Button variant="danger" onClick={stop}>
                Stop
              </Button>
              <Spinner label={phaseMessage || "Working…"} />
            </>
          ) : null}
          {workspace.validation?.ok ? (
            <span className="text-xs text-[var(--color-ink-muted)]">
              {workspace.validation.taskCount} tasks ready
              {signedIn ? "" : " · not signed in"}
            </span>
          ) : null}
        </div>
      </Card>

      {failure ? (
        <Alert tone="bad" title="The import stopped">
          {failure}
        </Alert>
      ) : null}

      {phase && phase !== "finished" ? (
        <Card>
          <SectionTitle
            title={PHASE_LABELS[phase]}
            subtitle={phaseMessage}
            right={
              progress ? (
                <span className="mono shrink-0 text-xs text-[var(--color-ink-muted)]">
                  {progress.index} / {progress.total}
                </span>
              ) : null
            }
          />
          <div className="h-1.5 w-full overflow-hidden rounded-full bg-[var(--color-surface-sunken)]">
            <div
              className="h-full rounded-full bg-[var(--color-accent)] transition-[width] duration-300"
              style={{ width: `${percent}%` }}
            />
          </div>
          {lines.length > 0 ? (
            <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-[var(--color-ink-muted)]">
              {Object.entries(counts).map(([status, n]) => (
                <span key={status}>
                  {STATUS_LABELS[status as TaskStatus]}: {n}
                </span>
              ))}
            </div>
          ) : null}
        </Card>
      ) : null}

      {(lines.length > 0 || log.length > 0) && !summary ? (
        <Card className="p-0">
          <div className="p-5 pb-3">
            <SectionTitle title="Live log" />
          </div>
          <div
            ref={logRef}
            className="mono max-h-80 overflow-auto border-t border-[var(--color-edge)] p-4 text-xs"
          >
            {log.map((m, i) => (
              <div key={`log-${i}`} className="whitespace-pre-wrap break-words text-[var(--color-ink-muted)]">
                {m}
              </div>
            ))}
            {lines.map((l) => (
              <div key={l.key} className="flex items-start gap-2 whitespace-pre-wrap break-words py-0.5">
                <span className="w-32 shrink-0">
                  <Badge tone={STATUS_TONE[l.status]}>{STATUS_LABELS[l.status]}</Badge>
                </span>
                <span className="w-32 shrink-0 pt-0.5">{l.id}</span>
                <span className="pt-0.5 text-[var(--color-ink-muted)]">{l.detail}</span>
              </div>
            ))}
          </div>
        </Card>
      ) : null}

      {summary ? <Results summary={summary} statuses={statuses} /> : null}

      {summary ? (
        <Card>
          <SectionTitle
            title="Compare with the CLI"
            subtitle="The exact text the Python importer prints for this backlog."
          />
          {cliOutput === null ? (
            <Button variant="ghost" onClick={showCliOutput}>
              Show CLI output
            </Button>
          ) : (
            <pre className="mono max-h-72 overflow-auto rounded-lg border border-[var(--color-edge)] bg-[var(--color-surface-sunken)] p-3 text-xs whitespace-pre-wrap">
              {cliOutput}
            </pre>
          )}
        </Card>
      ) : null}
    </div>
  );
}

function Results({
  summary,
  statuses,
}: {
  summary: ImportSummary;
  statuses: TaskIssueStatus[];
}) {
  const verb = summary.dryRun ? "would be created" : "created";
  const onProject = statuses.filter((s) => s.onProject).length;

  return (
    <Card>
      <SectionTitle
        title={summary.dryRun ? "Dry run finished" : "Import finished"}
        subtitle={
          summary.dryRun
            ? "Nothing on GitHub was changed."
            : "The state file records every issue, so re-running is safe."
        }
        right={<Badge tone={summary.dryRun ? "accent" : "good"}>{verb}</Badge>}
      />

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat label="Created" value={summary.created} />
        <Stat label="Already existed" value={summary.skipped} />
        <Stat label="Labels made" value={summary.labelsCreated.length} />
        <Stat label="On the board" value={summary.projectItemsAdded} />
      </div>

      {!summary.dryRun ? (
        <div className="mt-3 grid grid-cols-2 gap-3 sm:grid-cols-4">
          <Stat label="Links applied" value={summary.relationshipsApplied} />
          <Stat label="Links already set" value={summary.relationshipsAlready} />
          <Stat label="Links failed" value={summary.relationshipsFailed} />
          <Stat label="Issues tracked" value={statuses.length || summary.tracked} />
        </div>
      ) : null}

      {summary.labelsCreated.length > 0 ? (
        <div className="mt-5">
          <div className="mb-1.5 text-xs font-medium text-[var(--color-ink-muted)]">
            Labels created
          </div>
          <div className="flex flex-wrap gap-1.5">
            {summary.labelsCreated.map((l) => (
              <Badge key={l} tone="good">
                {l}
              </Badge>
            ))}
          </div>
        </div>
      ) : null}

      {summary.failedIds.length > 0 ? (
        <div className="mt-5">
          <Alert tone="bad" title={`${summary.failedIds.length} task(s) failed`}>
            <span className="mono">{summary.failedIds.join(", ")}</span> — re-run to retry; the
            ones that succeeded are skipped.
          </Alert>
        </div>
      ) : null}

      {summary.warnings.length > 0 ? (
        <div className="mt-5 space-y-2">
          {summary.warnings.map((w, i) => (
            <Alert key={i} tone="warn">
              {w}
            </Alert>
          ))}
        </div>
      ) : null}

      {statuses.length > 0 ? (
        <div className="mt-5">
          <div className="mb-1.5 text-xs font-medium text-[var(--color-ink-muted)]">
            {statuses.length} issues tracked{onProject > 0 ? `, ${onProject} on the board` : ""}
          </div>
          <div className="max-h-64 overflow-auto rounded-lg border border-[var(--color-edge)]">
            <table className="w-full border-collapse text-sm">
              <thead className="sticky top-0 bg-[var(--color-surface-raised)]">
                <tr className="text-left text-xs text-[var(--color-ink-muted)]">
                  <th className="px-4 py-2 font-medium">Task</th>
                  <th className="px-3 py-2 font-medium">Issue</th>
                  <th className="px-4 py-2 font-medium">Board</th>
                </tr>
              </thead>
              <tbody>
                {statuses.map((s) => (
                  <tr key={s.id} className="border-t border-[var(--color-edge)]">
                    <td className="mono px-4 py-2">{s.id}</td>
                    <td className="mono px-3 py-2">#{s.number}</td>
                    <td className="px-4 py-2">
                      {s.onProject ? <Badge tone="good">yes</Badge> : <Badge>no</Badge>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}
    </Card>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg border border-[var(--color-edge)] bg-[var(--color-surface-sunken)] px-3 py-2.5">
      <div className="mono text-xl font-semibold">{value}</div>
      <div className="text-xs text-[var(--color-ink-muted)]">{label}</div>
    </div>
  );
}
