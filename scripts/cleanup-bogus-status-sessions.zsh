#!/usr/bin/env zsh

emulate -L zsh
setopt errexit nounset pipefail

codex_home=${CODEX_HOME:-$HOME/.codex}
state_db=$codex_home/state_5.sqlite
logs_db=$codex_home/logs_2.sqlite
session_index=$codex_home/session_index.jsonl
history_file=$codex_home/history.jsonl
backup_root=$codex_home/cleanup-backups/bogus-status-sessions-$(date +%Y%m%d-%H%M%S)

dry_run=0
if [[ ${1:-} == "--dry-run" ]]; then
  dry_run=1
elif [[ $# -gt 0 ]]; then
  print -u2 "usage: $0 [--dry-run]"
  exit 2
fi

if [[ ! -f $state_db ]]; then
  print -u2 "missing state database: $state_db"
  exit 1
fi

if ! command -v sqlite3 >/dev/null 2>&1; then
  print -u2 "sqlite3 is required"
  exit 1
fi

mkdir -p $backup_root

candidate_tsv=$backup_root/candidates.tsv
candidate_ids=$backup_root/candidate-ids.txt

sqlite3 -batch -separator $'\t' $state_db <<'SQL' >$candidate_tsv
WITH fragments(value) AS (
    VALUES ('status'), ('tatus$'), ('atus'), ('tus'), ('us'), ('s')
),
candidates AS (
    SELECT
        id,
        rollout_path,
        title,
        first_user_message,
        updated_at
    FROM threads
    WHERE archived = 0
      AND (
          trim(first_user_message) IN (SELECT value FROM fragments)
          OR trim(title) IN (SELECT value FROM fragments)
          OR trim(first_user_message) IN (SELECT value || '/status' FROM fragments)
          OR trim(title) IN (SELECT value || '/status' FROM fragments)
          OR trim(first_user_message) = '$github tatus/status'
          OR trim(title) = '$github tatus/status'
      )
)
SELECT id, rollout_path, title, first_user_message, updated_at
FROM candidates
ORDER BY updated_at DESC;
SQL

ids=()
rollout_paths=()
while IFS=$'\t' read -r id rollout_path _title _first_user_message _updated_at; do
  [[ -z ${id:-} ]] && continue
  ids+=($id)
  [[ -n ${rollout_path:-} ]] && rollout_paths+=($rollout_path)
done <$candidate_tsv

if (( ${#ids[@]} == 0 )); then
  print "No active bogus status sessions found."
  print "Candidate report: $candidate_tsv"
  exit 0
fi

print -l -- $ids >$candidate_ids

print "Found ${#ids[@]} bogus status session(s):"
while IFS=$'\t' read -r id _rollout_path title first_user_message updated_at; do
  printf '  %s  title=%q first=%q updated_at=%s\n' $id $title $first_user_message $updated_at
done <$candidate_tsv

if (( dry_run )); then
  print "Dry run only. Candidate report: $candidate_tsv"
  exit 0
fi

for file in $state_db $state_db-wal $state_db-shm $logs_db $logs_db-wal $logs_db-shm $session_index $history_file; do
  [[ -e $file ]] || continue
  cp -p $file $backup_root/${file:t}
done

sql_list="'${(j:',':)ids}'"

sqlite3 -batch $state_db <<SQL
PRAGMA foreign_keys = ON;
BEGIN IMMEDIATE;
DELETE FROM thread_dynamic_tools WHERE thread_id IN ($sql_list);
DELETE FROM thread_goals WHERE thread_id IN ($sql_list);
DELETE FROM thread_spawn_edges WHERE parent_thread_id IN ($sql_list) OR child_thread_id IN ($sql_list);
DELETE FROM threads WHERE id IN ($sql_list);
COMMIT;
VACUUM;
SQL

if [[ -f $logs_db ]]; then
  sqlite3 -batch $logs_db <<SQL
BEGIN IMMEDIATE;
DELETE FROM logs WHERE thread_id IN ($sql_list);
COMMIT;
VACUUM;
SQL
fi

prune_jsonl() {
  local file=$1
  [[ -f $file ]] || return 0

  local tmp=$backup_root/${file:t}.tmp
  local line id keep_line

  >$tmp
  while IFS= read -r line; do
    keep_line=1
    for id in $ids; do
      if [[ $line == *$id* ]]; then
        keep_line=0
        break
      fi
    done

    (( keep_line )) && print -r -- $line >>$tmp
  done <$file

  mv $tmp $file
}

prune_jsonl $session_index
prune_jsonl $history_file

rollout_backup=$backup_root/rollouts
mkdir -p $rollout_backup
for rollout_path in $rollout_paths; do
  [[ -f $rollout_path ]] || continue
  cp -p $rollout_path $rollout_backup/${rollout_path:t}
  mv $rollout_path $rollout_backup/${rollout_path:t}.removed
done

print "Removed ${#ids[@]} bogus status session(s)."
print "Backup: $backup_root"
