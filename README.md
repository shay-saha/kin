# Kin

Kin brings a family's photos, recordings, and stories together. A loved one can
ask “Who is this?” and hear a short, grounded cue when independent family
memories support it. Otherwise Kin stays quiet. This is a hackathon prototype
for reminiscence support, not a medical device.

## Current app

- **Memories** (`/family`): photos with explicit face labels, previewable voice
  recordings, Weaver questions, and review of a loved one's stories.
- **Living Stories** (`/stories`): main’s continuous, source-attributed listening
  experience, timed passages, topic contributions, and Quiet view.
- **Pull-apart stories**: unfold a shared memory into relatives' perspectives;
  inspect its sources, play original recordings, and answer a missing connection.
- **Recognize** (`/wearer`): camera access on request and one recognition action.
- **Connections** (`/stage`): recall status, evidence, family connections, and replay.
- **Memory Atlas** (`/graph`): main's interactive graph and evidence trails.
- **My recordings** (`/remember`): the loved one's own stories, optionally held for
  family review. Reach it from Recognize.
- **Accounts**: signup, confirmation/reset callbacks, empty-family onboarding,
  contributor invitations, a separate loved-one invitation, and Settings.
  Organizer-only sample/reset tools live in Settings and require an explicit action.

The full interface uses the Rust face pipeline, atomic ingestion, two-Keeper
evidence policy, and account provisioning.
See [VERIFICATION.md](VERIFICATION.md) for what was tested.

## Run locally

```bash
npm ci
cp .env.example .env.local
# Fill in .env.local and apply the Supabase migrations below.
npm run faces:models
npm run dev
```

Open <http://localhost:3000>. Rust 1.96 or later is required. `npm run dev` starts
the Rust API on port 8787 and Next.js on port 3000. Next.js forwards `/api/*` and
`/auth/callback` to Rust. For production, run `npm run build:backend` and
`npm run build`, then `npm start`. Camera/microphone access needs localhost or HTTPS; use an HTTPS
address when testing on a physical phone.

Required configuration is listed in `.env.example`: Supabase URL, browser
publishable/anon key, server secret/service-role key, OpenAI, Deepgram, and
ElevenLabs (including a voice ID). OpenAI supplies extraction, descriptive
captions and semantic embeddings; Deepgram transcribes recordings with family-name
hints (Nova-3 keyterm prompting when names are available); ElevenLabs
voices cues, with browser speech as the client fallback. The active backend
uses OpenAI; the earlier feature branch's Anthropic-only mode is not active.

Face detection and recognition run in Rust using YuNet, SFace, and the Tract
ONNX runtime. `npm run faces:models` downloads the pinned model artifacts to
ignored `brain/models`. Model licenses are retained in `brain/licenses`.
Set `KIN_FACE_TOKEN_KEY` to a separate 32-byte random key encoded as
64 hexadecimal characters. Image uploads retain their original bytes; explicit
consent and labels are required for enrollment. Human memories from at least
two distinct Keepers must support a cue, and `KIN_GATE_THRESHOLD` cannot lower
the 0.85 minimum. Scores remain heuristics.

Apply migration `011_rust_face_models.sql` when upgrading an existing database.
Existing face vectors retain their original model and are not converted. Repeat
face enrollment for Rust-native recognition. An external compatible inference
endpoint can be selected with `KIN_FACE_SERVICE_URL`, `KIN_FACE_SERVICE_TOKEN`,
and `KIN_FACE_MODEL`; the legacy model identifier is listed in `.env.example`.

The backend reads `.env.local` when launched from the repository root. Use
`KIN_BRAIN_PORT` and `KIN_BACKEND_BIND` to configure its listener. Set
`KIN_BACKEND_URL` before building and running Next.js when the backend runs on
another host. Setting that URL makes the app launcher use the external backend.
Run `npm run dev:web` or `npm run dev:backend` to start each service separately.
Server secrets belong only in the Rust service environment when deploying the
services separately. Supabase Auth, PostgreSQL, storage, and browser realtime
subscriptions keep their existing contracts.

## Supabase setup and upgrades

For a **new database**, apply `supabase/migrations/001_init.sql` through
`011_rust_face_models.sql` in numeric order. For an **existing main database**
with 001–008 already applied, apply only:

1. `009_family_accounts.sql`
2. `010_loved_one_invites.sql`
3. `011_rust_face_models.sql`

If 008 has not been applied yet, apply `008_living_stories.sql` first.

These add private family accounts and invitations, backfill existing provisioned
memberships, and connect new loved-one accounts to personal recording/review.
They do not reset memories or accounts. Their reapplication is tested locally.
Do not blindly rerun the earlier migrations or mix in the feature branch's old,
conflicting migration numbers. If your database was created from the older
pull-apart branch (for example, 010 reports that `wearer_accounts` is missing),
run the entire [legacy recovery SQL](supabase/upgrades/pull_apart_to_main.sql)
in Supabase SQL Editor instead. It upgrades that schema through 011 in one
transaction, preserves existing data, and skips demo consolidation. It is also
safe after applying 009 and seeing 010 fail. Sign out and back in afterward.
See [recovery details](supabase/upgrades/README.md).
No hosted database migration or reset is performed by installing/building the app.

In Supabase Auth, enable email/password and configure the app's Site URL plus
`http://localhost:3000/auth/callback` and your deployed HTTPS callback URL as
allowed redirects. Sign up, create a family, then invite contributors or the
loved one from Settings. Confirmation/reset email delivery needs a configured
Supabase mail provider and a physical end-to-end check.

Existing canonical demo accounts remain supported. The optional
`npm run demo:provision` and `npm run seed` use the real configured database.
Provisioning requires `KIN_DEMO_PASSWORD` and preserves existing account passwords;
seeding adds only missing canonical fictional records.
Seeding is not required for a new family. `/graph?demo=1` displays bundled fictional
Atlas data without accessing a private family's memories.

## Verification

```bash
npm test
npm run lint
npm run build
npx tsc --noEmit --incremental false
npm ci --prefix tests/database
npm run test:db
npx playwright install chromium
npm run test:e2e
cargo fmt --check --manifest-path brain/Cargo.toml
cargo clippy --locked --manifest-path brain/Cargo.toml --all-targets -- -D warnings
```

`test:db` runs disposable PostgreSQL WASM with pgvector. `test:e2e` starts a
loopback-only Supabase HTTP test double on 54329, Rust on 8788, and Next.js on 3102; it overrides
real credentials and cleans up its servers. Keep those ports free and do not
run another Next.js process in the same checkout during the browser suite.
Pass Playwright filters after `--`. An existing Chromium binary can be selected
with `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH`. The browser double tests application
flows, not hosted RLS or real provider accuracy; database rules are checked by
the separate SQL suite.

All application backend code is Rust in `brain/src`, including accounts,
provider integrations, ingestion, face inference, recall, speech, Weaver,
self-review, deletion, and demo administration. TypeScript contains the web and
mobile interfaces and development verification tools. The Rust tests exercise
HTTP workflows against isolated provider/database doubles and preserve the
original gate, Keeper, and Weaver conformance fixtures. SQL tests verify the
actual database transactions and access rules. Rust source is comment-free and
uses named types and functions to express behavior.

## Demo narration

To add stock ElevenLabs narration to the six existing fictional demo stories:

```bash
npm run demo:audio          # Preview eligible stories and character count
npm run demo:audio -- --apply
```

This uses `ELEVENLABS_API_KEY` and server Supabase credentials from `.env.local`.
Maya, David, and Elena each have a distinct stock voice. Only unchanged scripts
from `demo/shared-baseline.json` without audio are selected; accounts, user
recordings, photos, and face enrollments are preserved. Clips are cached under
ignored `demo/fixtures/private/narration` and stored privately in Supabase.
Sentence timestamps support Living Stories excerpts. Generated clips display
“Demo narration” and are not treated as human evidence for recognition cues.
Repeating the command skips stories that already have audio.
