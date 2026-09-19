import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/api";
import type { AppInfo, Config, Workspace } from "../lib/types";
import { Alert, Badge, Button, Card, Field, SectionTitle, TextInput } from "./ui";

const EMPTY: Config = { repo: "", project: "", project_owner: "" };

/**
 * The workspace: which repository, which project board, and where the files
 * live. `config.json` is shared byte-for-byte with the Python CLI, so the
 * field names keep its snake_case `project_owner` key.
 */
export function ConfigPanel({
  workspace,
  appInfo,
  onChanged,
}: {
  workspace: Workspace;
  appInfo: AppInfo | null;
  onChanged: (w: Workspace) => void;
}) {
  const [draft, setDraft] = useState<Config>(workspace.config ?? EMPTY);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // Adopt whatever was loaded from disk (or from a file the user picked).
  useEffect(() => {
    setDraft(workspace.config ?? EMPTY);
  }, [workspace.config]);

  const dirty =
    draft.repo !== (workspace.config?.repo ?? "") ||
    draft.project !== (workspace.config?.project ?? "") ||
    draft.project_owner !== (workspace.config?.project_owner ?? "");

  async function save() {
    setError(null);
    setBusy(true);
    try {
      onChanged(await api.saveConfig({ ...draft, repo: draft.repo.trim() }));
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <SectionTitle
        title="Workspace"
        subtitle="Where the issues go, and where this app keeps its files."
        right={
          appInfo ? (
            <Badge tone={appInfo.portable ? "accent" : "neutral"}>
              {appInfo.portable ? "Portable" : "Installed"}
            </Badge>
          ) : null
        }
      />

      {workspace.configError ? (
        <div className="mb-4">
          <Alert tone="bad" title="config.json could not be read">
            {workspace.configError}
          </Alert>
        </div>
      ) : null}

      {error ? (
        <div className="mb-4">
          <Alert tone="bad">{error}</Alert>
        </div>
      ) : null}

      <div className="space-y-4">
        <Field label="Repository" hint="Owner and name, for example PersianRepo/front.">
          <TextInput
            value={draft.repo}
            onChange={(v) => setDraft({ ...draft, repo: v })}
            placeholder="owner/repo"
            spellCheck={false}
          />
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field
            label="Project name"
            hint="Leave both project fields empty to skip the board entirely."
          >
            <TextInput
              value={draft.project}
              onChange={(v) => setDraft({ ...draft, project: v })}
              placeholder="Front"
            />
          </Field>
          <Field label="Project owner" hint="The user or organisation that owns the board.">
            <TextInput
              value={draft.project_owner}
              onChange={(v) => setDraft({ ...draft, project_owner: v })}
              placeholder="PersianRepo"
              spellCheck={false}
            />
          </Field>
        </div>

        <div className="flex items-center gap-3">
          <Button variant="primary" onClick={save} disabled={busy || !dirty}>
            {busy ? "Saving…" : saved ? "Saved" : "Save config"}
          </Button>
          {dirty ? (
            <span className="text-xs text-[var(--color-ink-muted)]">Unsaved changes</span>
          ) : null}
        </div>
      </div>

      <dl className="mt-6 space-y-1.5 border-t border-[var(--color-edge)] pt-4 text-xs">
        <PathRow label="Folder" value={appInfo?.baseDir} />
        <PathRow label="Config" value={appInfo?.configPath} />
        <PathRow label="Backlog" value={appInfo?.tasksPath} />
        <PathRow
          label="State"
          value={appInfo?.statePath}
          note={appInfo?.stateExists ? "resuming from a previous run" : "no previous run"}
        />
      </dl>

      <div className="mt-3">
        <Button variant="ghost" onClick={() => void api.revealWorkspace()}>
          Open folder
        </Button>
      </div>
    </Card>
  );
}

function PathRow({
  label,
  value,
  note,
}: {
  label: string;
  value?: string;
  note?: string;
}) {
  if (!value) return null;
  return (
    <div className="flex gap-3">
      <dt className="w-16 shrink-0 text-[var(--color-ink-muted)]">{label}</dt>
      <dd className="mono min-w-0 flex-1 break-all">{value}</dd>
      {note ? (
        <dd className="shrink-0 text-[var(--color-ink-muted)]">{note}</dd>
      ) : null}
    </div>
  );
}
