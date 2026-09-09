#!/usr/bin/env python3
import argparse, json, re, shutil, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CONFIG = ROOT / "config.json"
TASKS = ROOT / "tasks.json"
STATE = ROOT / ".import-state.json"


def gh(*args, check=True):
    p = subprocess.run(["gh", *args], text=True, capture_output=True)
    if check and p.returncode:
        print(f"\nGitHub CLI failed: gh {' '.join(args)}", file=sys.stderr)
        if p.stdout: print(p.stdout)
        if p.stderr: print(p.stderr, file=sys.stderr)
        raise SystemExit(p.returncode)
    return p.stdout.strip()


def load(path):
    try: return json.loads(path.read_text(encoding="utf-8"))
    except Exception as e: raise SystemExit(f"Cannot read {path}: {e}")


def validate(tasks):
    ids=set(); errors=[]
    for t in tasks:
        for k in ("id","module","title","priority","labels","body","depends_on","parent"):
            if k not in t: errors.append(f"{t.get('id','?')}: missing {k}")
        tid=t.get("id")
        if tid in ids: errors.append(f"{tid}: duplicate ID")
        ids.add(tid)
        if t.get("priority") not in {"P0","P1","P2","P3"}: errors.append(f"{tid}: invalid priority")
    for t in tasks:
        tid=t["id"]
        for d in t["depends_on"]:
            if d not in ids: errors.append(f"{tid}: unknown dependency {d}")
            if d==tid: errors.append(f"{tid}: self dependency")
        if t["parent"] and t["parent"] not in ids: errors.append(f"{tid}: unknown parent {t['parent']}")
    if errors:
        print("Validation failed:")
        print("\n".join(f"- {e}" for e in errors)); raise SystemExit(1)
    print(f"OK: {len(tasks)} tasks validated.")


def existing(repo):
    raw=gh("issue","list","--repo",repo,"--state","all","--limit","1000","--json","number,title")
    found={}
    for x in json.loads(raw or "[]"):
        m=re.match(r"^\[([A-Z0-9]+(?:-[A-Z0-9]+)*-\d{3})\]\s",x["title"])
        if m: found[m.group(1)]=int(x["number"])
    return found


def ensure_labels(repo,tasks):
    needed={"backend","P0","P1","P2","P3"}
    for t in tasks: needed|={t["module"],*t["labels"]}
    raw=gh("label","list","--repo",repo,"--limit","1000","--json","name")
    have={x["name"] for x in json.loads(raw or "[]")}
    for label in sorted(needed-have):
        print(f"+ label {label}"); gh("label","create",label,"--repo",repo)


def project_check(owner, name):
    raw = gh("project", "list", "--owner", owner, "--format", "json", "--limit", "1000")
    data = json.loads(raw or "[]")

    if isinstance(data, dict):
        projects = data.get("projects", [])
    else:
        projects = data

    titles = {
        x.get("title")
        for x in projects
        if isinstance(x, dict)
    }

    if name not in titles:
        raise SystemExit(
            f"Project '{name}' not found for owner '{owner}'. "
            f"Available projects: {sorted(titles)}"
        )

def create_issue(repo,project,t,relationships=False,imap=None):
    labels=list(dict.fromkeys(["backend",t["module"],t["priority"],*t["labels"]]))
    body=f"<!-- gitrise-task:{t['id']} -->\n\n**Module:** `{t['module']}`  \n**Priority:** `{t['priority']}`\n\n{t['body']}\n\n## Acceptance Criteria\n\n- [ ] Implementation completed\n- [ ] Appropriate tests added\n- [ ] API/documentation updated where applicable\n"
    args=["issue","create","--repo",repo,"--title",f"[{t['id']}] {t['title']}","--body",body,"--label",",".join(labels),"--project",project]
    if relationships and imap:
        if t.get("parent"): args += ["--parent",str(imap[t["parent"]])]
        for d in t["depends_on"]: args += ["--blocked-by",str(imap[d])]
    out=gh(*args)
    m=re.search(r"/issues/(\d+)",out)
    if not m: raise SystemExit(f"Could not parse issue number from: {out}")
    return int(m.group(1))


def main():
    ap=argparse.ArgumentParser(); ap.add_argument("command",choices=["validate","preview","create","sync"]); a=ap.parse_args()
    c=load(CONFIG); tasks=load(TASKS); validate(tasks)
    if a.command=="validate": return
    if a.command=="preview":
        for t in tasks: print(f"{t['id']:<15} {t['priority']} {t['module']:<18} {t['title']}")
        return
    if not shutil.which("gh"): raise SystemExit("GitHub CLI 'gh' is not installed or not on PATH.")
    gh("auth","status"); gh("repo","view",c["repo"],"--json","nameWithOwner"); project_check(c["project_owner"],c["project"])
    ensure_labels(c["repo"],tasks); imap=existing(c["repo"])
    created=0
    for t in tasks:
        if t["id"] in imap: print(f"= {t['id']} -> #{imap[t['id']]}"); continue
        print(f"+ creating {t['id']}"); imap[t["id"]]=create_issue(c["repo"],c["project"],t); created+=1
        STATE.write_text(json.dumps({"issues":imap},indent=2),encoding="utf-8")
    # Relationships are applied in a second pass so task order cannot break references.
    for t in tasks:
        n=imap[t["id"]]
        if t.get("parent"): gh("issue","edit",str(n),"--repo",c["repo"],"--parent",str(imap[t["parent"]]))
        if t["depends_on"]: gh("issue","edit",str(n),"--repo",c["repo"],"--add-blocked-by",",".join(str(imap[d]) for d in t["depends_on"]))
    STATE.write_text(json.dumps({"issues":imap},indent=2),encoding="utf-8")
    print(f"\nDone. Created {created}; tracked {len(imap)} tasks.")

if __name__=="__main__": main()
