# Upgrade the original pull-apart database

Use `pull_apart_to_main.sql` when a database was created from the original
`feature/pull-apart-stories` migrations (`001_init`, `002_demo_reliability`,
`003_accounts`, `004_loved_one_invites`). These numbers do not correspond to
main's historical migrations.

The characteristic failures are `010: relation public.wearer_accounts does not
exist` and “Your family couldn't be loaded.” The merged app also needs ingestion
receipts, source/fact columns, recall evidence fields, and self-contribution tables;
creating just `wearer_accounts` does not fix the whole schema.

1. Open `pull_apart_to_main.sql` and copy the entire file into Supabase SQL Editor.
2. Run it as one query. It contains its own transaction and schema-cache reload.
3. Sign out of Kin and sign back in to refresh account claims, then reload Memories.

This file covers main's schema changes through 011, including the Rust face-model update. It may be
run after 009 succeeded and 010 failed, and may be retried after a successful run.
It skips 005, which consolidates demo data. No passwords, accounts, memory rows,
media objects, original transcripts, IDs, or stored face vectors are deleted.
Existing loved-one memberships get a self Keeper with family review enabled;
existing capture preferences remain unchanged.

Legacy facts and face descriptors retain their unverified/model-less state. The
library stays visible, but recognition needs newly enrolled faces and supported
human facts accepted by the current backend. The upgrade does not invent evidence
or automatically spend provider credits reprocessing old contributions.

Unexpected duplicate answers/open questions or conflicting wearer memberships
abort the transaction for review instead of silently deleting records. No reset
or seed is needed. This script is for a database with the old account schema;
a new database should use the numbered migrations normally.

The file is generated from the current migrations with retry-safe DDL and a
membership bridge. Regenerate after changing those migrations:

```bash
node scripts/build-legacy-upgrade.mjs
npm run test:db
```

The regression test recreates the original branch schema, reproduces the failed
010 upgrade, applies recovery, checks original data and browser access isolation,
and runs recovery a second time. It never connects to hosted Supabase.
