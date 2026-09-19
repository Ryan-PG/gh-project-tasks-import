import { useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage } from "../lib/api";
import type { Workspace } from "../lib/types";
import { Alert, Badge, Button, Card, SectionTitle, TextInput } from "./ui";

const PAGE = 250;

/**
 * Loading and validating the backlog.
 *
 * Validation mirrors the Python CLI exactly — including the order of the error
 * lines — so a backlog the CLI accepts is accepted here, and one it rejects is
 * rejected with the same message.
 */
export function BacklogPanel({
  workspace,
  onChanged,
}: {
  workspace: Workspace;
  onChanged: (w: Workspace) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [showAllRows, setShowAllRows] = useState(false);

  const report = workspace.validation;
  const rows = workspace.preview;

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter(
      (r) =>
        r.id.toLowerCase().includes(q) ||
        r.title.toLowerCase().includes(q) ||
        r.module.toLowerCase().includes(q) ||
        r.labels.some((l) => l.toLowerCase().includes(q)),
    );
  }, [rows, filter]);

  const visible = showAllRows ? filtered : filtered.slice(0, PAGE);

  async function pickFile() {
    setError(null);
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        title: "Choose a tasks.json backlog",
        filters: [{ name: "Task backlog", extensions: ["json"] }],
      });
      if (typeof picked !== "string") return;
      setBusy(true);
      onChanged(await api.loadTasksFromPath(picked));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function reload() {
    setError(null);
    setBusy(true);
    try {
      onChanged(await api.loadWorkspace());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const source = workspace.tasksSource;

  return (
    <div className="space-y-5">
      <Card>
        <SectionTitle
          title="Backlog"
          subtitle={
            source
              ? `${workspace.taskCount} task${workspace.taskCount === 1 ? "" : "s"} from ${source}`
              : "No backlog loaded yet."
          }
          right={
            <div className="flex shrink-0 gap-2">
              <Button onClick={reload} disabled={busy}>
                Reload
              </Button>
              <Button variant="primary" onClick={pickFile} disabled={busy}>
                {busy ? "Reading…" : "Choose file…"}
              </Button>
            </div>
          }
        />

        {error ? (
          <div className="mb-4">
            <Alert tone="bad">{error}</Alert>
          </div>
        ) : null}

        {workspace.tasksError ? (
          <div className="mb-4">
            <Alert tone="bad" title="The backlog could not be read">
              {workspace.tasksError}
            </Alert>
          </div>
        ) : null}

        {report?.ok ? (
          <Alert tone="good" title="Validation passed">
            {report.taskCount} task{report.taskCount === 1 ? "" : "s"} checked, no problems
            found.
          </Alert>
        ) : null}

        {report && !report.ok ? (
          <div>
            <Alert tone="bad" title={`Validation failed with ${report.errors.length} problem${report.errors.length === 1 ? "" : "s"}`}>
              Fix these in the backlog file, then reload. Nothing is sent to GitHub until
              validation passes.
            </Alert>
            <ul className="mono mt-3 max-h-72 space-y-1 overflow-auto rounded-lg border border-[var(--color-edge)] bg-[var(--color-surface-sunken)] p-3 text-xs">
              {report.errors.map((e, i) => (
                <li key={`${i}-${e}`} className="whitespace-pre-wrap break-words">
                  {e}
                </li>
              ))}
            </ul>
          </div>
        ) : null}

        <SettingsBanner workspace={workspace} />
      </Card>

      {report?.ok && rows.length > 0 ? (
        <Card className="p-0">
          <div className="flex flex-wrap items-center justify-between gap-3 p-5 pb-4">
            <SectionTitle
              title="Preview"
              subtitle="Exactly the issues this run would create."
            />
            <div className="w-56 shrink-0">
              <TextInput value={filter} onChange={setFilter} placeholder="Filter…" />
            </div>
          </div>

          <div className="overflow-x-auto border-t border-[var(--color-edge)]">
            <table className="w-full border-collapse text-sm">
              <thead>
                <tr className="text-left text-xs text-[var(--color-ink-muted)]">
                  <th className="px-5 py-2.5 font-medium">ID</th>
                  <th className="px-3 py-2.5 font-medium">Priority</th>
                  <th className="px-3 py-2.5 font-medium">Module</th>
                  <th className="px-3 py-2.5 font-medium">Title</th>
                  <th className="px-3 py-2.5 font-medium">Labels</th>
                  <th className="px-5 py-2.5 font-medium">Relations</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((r) => (
                  <tr
                    key={r.id}
                    className="border-t border-[var(--color-edge)] align-top"
                  >
                    <td className="mono whitespace-nowrap px-5 py-2.5">{r.id}</td>
                    <td className="px-3 py-2.5">{r.priority}</td>
                    <td className="px-3 py-2.5">{r.module}</td>
                    <td className="px-3 py-2.5">{r.title}</td>
                    <td className="px-3 py-2.5">
                      <div className="flex flex-wrap gap-1">
                        {r.labels.map((l) => (
                          <Badge key={l}>{l}</Badge>
                        ))}
                      </div>
                    </td>
                    <td className="px-5 py-2.5 text-xs text-[var(--color-ink-muted)]">
                      {r.parent ? (
                        <div>
                          parent <span className="mono">{r.parent}</span>
                        </div>
                      ) : null}
                      {r.dependsOn.length > 0 ? (
                        <div>
                          after{" "}
                          <span className="mono">{r.dependsOn.join(", ")}</span>
                        </div>
                      ) : null}
                      {!r.parent && r.dependsOn.length === 0 ? "—" : null}
                    </td>
                  </tr>
                ))}
                {visible.length === 0 ? (
                  <tr className="border-t border-[var(--color-edge)]">
                    <td
                      colSpan={6}
                      className="px-5 py-6 text-center text-[var(--color-ink-muted)]"
                    >
                      Nothing matches “{filter}”.
                    </td>
                  </tr>
                ) : null}
              </tbody>
            </table>
          </div>

          {filtered.length > visible.length ? (
            <div className="border-t border-[var(--color-edge)] p-4 text-center">
              <Button variant="ghost" onClick={() => setShowAllRows(true)}>
                Show all {filtered.length} rows
              </Button>
            </div>
          ) : null}
        </Card>
      ) : null}
    </div>
  );
}

/** The two environment-driven switches the CLI honours, and where each came from. */
function SettingsBanner({ workspace }: { workspace: Workspace }) {
  const { settings, settingsBanner } = workspace;
  const notable = settings.skipRelationships || settings.relationshipErrors === "ignore";

  return (
    <div className="mt-4">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className="text-[var(--color-ink-muted)]">
          <span className="mono">RELATIONSHIP_ERRORS</span>
        </span>
        <Badge tone={settings.relationshipErrors === "strict" ? "neutral" : "warn"}>
          {settings.relationshipErrors}
        </Badge>
        <span className="text-[var(--color-ink-muted)]">from {settings.relationshipErrorsSource}</span>

        <span className="ml-3 text-[var(--color-ink-muted)]">
          <span className="mono">SKIP_RELATIONSHIPS</span>
        </span>
        <Badge tone={settings.skipRelationships ? "warn" : "neutral"}>
          {settings.skipRelationships ? "true" : "false"}
        </Badge>
        <span className="text-[var(--color-ink-muted)]">from {settings.skipRelationshipsSource}</span>
      </div>

      {/* Unset or unrecognised values. The CLI prints these to stderr; showing
          them here is how the app explains a value it silently ignored. */}
      {settings.warnings.length > 0 ? (
        <ul className="mono mt-2 space-y-1 text-xs text-amber-700 dark:text-amber-400">
          {settings.warnings.map((w, i) => (
            <li key={i}>{w}</li>
          ))}
        </ul>
      ) : null}

      {notable && settingsBanner ? (
        <div className="mt-2">
          <Alert tone="warn">{settingsBanner}</Alert>
        </div>
      ) : null}
    </div>
  );
}
