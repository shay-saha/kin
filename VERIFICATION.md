# Rust backend verification

The application backend runs in `brain/src`. Next.js forwards application API
requests and authentication callbacks to Rust. Supabase remains responsible for
authentication, PostgreSQL transactions, access policies, storage, and realtime.
Migration 011 allows versioned Rust-native face enrollments alongside legacy
descriptors. Existing records are preserved; native recognition requires repeat
enrollment because descriptors from different models are not interchangeable.

## Automated checks

- 21 Rust integration tests exercise ingestion, authorization, callbacks, face
  selection sealing, self-review, grounded recall, deletion, Weaver routing,
  administration, and domain boundaries. The three conformance tests include
  83 original gate, Keeper, and Weaver fixture cases.
- 30 web unit tests pass.
- 26 disposable database checks pass using PostgreSQL WASM and real pgvector,
  including native/legacy enrollment restrictions, transactions, isolation,
  invitations, self-review, migration reapplication, and legacy schema recovery.
- 64 tracked Chromium browser scenarios pass using Rust behind Next.js and an
  isolated Supabase HTTP double.
- Rust formatting, strict Clippy, release compilation, TypeScript checking,
  frontend lint, and the production Next.js build pass.
- Native YuNet/SFace inference was run on a sample image and detected three
  faces with 128-value descriptors. Downloaded artifacts are checksum verified;
  model licenses are retained in `brain/licenses`.

Provider and HTTP integration tests use isolated doubles with synthetic
credentials. Browser scenarios exercise navigation, account flows, contribution
ownership, enrollment retries, recording preview, silence, replay, review,
organizer controls, story gestures, keyboard access, large text, and accessibility.
SQL tests check database behavior separately. See README for reproducible commands.
The pre-existing untracked photo-enrollment draft is not part of the tracked suite.

## Checks requiring deployed services or devices

- Confirmation and password-reset email delivery with deployed callback URLs.
- Paid transcription, extraction, embeddings, and speech with live providers.
- Recognition accuracy with consented enrolled and unknown faces under varied
  lighting, camera positions, and image quality.
- Camera, microphone, gestures, and audio output on physical phones.

No hosted database migration, demo provisioning, reset, or deployment was performed.
