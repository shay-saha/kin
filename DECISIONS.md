# Design decisions

## Rust backend

- Rust owns every application API and auth callback; Next.js forwards requests.
- Axum and Tokio serve the existing HTTP contracts. Rust calls Supabase Auth,
  PostgREST, private storage, OpenAI, Deepgram, and ElevenLabs directly.
- YuNet and SFace run through Tract with a distinct versioned face model. Existing
  vectors retain their model; migration 011 allows native enrollment. Recognition
  uses only descriptors from the selected model, so native adoption requires
  repeat enrollment.
- Atomic ingestion and self-review continue through the existing database RPCs.
  SQL migrations and RLS remain database responsibilities.
- Runtime Rust source contains no comments. Conformance fixtures, HTTP tests,
  database integration checks, and browser workflows verify the migration.

## Integration with main

- Preserve main's server-owned, versioned face inference, sealed enrollment
  selections, atomic/idempotent ingestion, human provenance, and conservative
  gate. The full redesigned interface uses those contracts.
- Supabase sessions use shared browser cookies. APIs verify users server-side;
  main's bearer-token clients remain supported. New family rows and existing
  admin-provisioned accounts can both use the interface.
- New account migrations are numbered 009/010 after main's 001–007. They preserve
  existing data, restrict private family reads, and keep biometric descriptors
  and retrieval RPCs server-only.
- A loved-one membership is distinct from contributor membership. Main's self
  Keeper enables their personal recordings, with family review by default for
  newly invited loved ones. Loved ones cannot invite members or use admin tools.
- Keep Memory Atlas, the self-review queue, organizer demo controls, Rust sources,
  conformance fixtures, and canonical demo provisioning from main.

## Recognition and evidence

- Identity comes from server face matching, never image-caption guessing.
  Photos retain original bytes and enrollment requires explicit permission.
- Main's 0.85 minimum, two independent supporting Keepers, literal fact grounding,
  contradiction handling and latest retrieval calibration remain unchanged.
- OpenAI handles descriptive captions, extraction and embeddings. Deepgram
  transcribes; ElevenLabs provides speech. No Anthropic-only embedding fallback
  is introduced because main's retrieval depends on real semantic vectors.
- Replay selects the latest successful event and evaluates it against current
  memories, even if a more recent attempt was silent. New attempts clear prior
  cues; leaving the screen cancels pending playback and speech fallback.
- Removing a contribution invalidates cached recall evidence and retry receipts
  and preserves graph facts still supported by another memory. Storage and SQL
  cleanup still span multiple operations; the memory remains until cleanup
  succeeds so failures can be retried.

## Interface and graph

- System typography, neutral surfaces, desktop navigation and mobile tabs follow
  the supplied design guidance. Technical details stay behind optional controls.
- Pull-apart stories retain original sources and opt-in audio. Sheets support
  pointer tracking, interruptible springs, keyboard alternatives and reduced motion.
- Recording requires stop, preview, then save. Camera/microphone permissions
  begin with a user action. Large text, contrast and reduced transparency are
  part of browser checks; physical devices still need manual testing.
- The Connections map has an accessible list; the separate React Flow v11 Atlas
  keeps main's richer exploration and memory-formation view.
- Main's Weaver routing, photo tie-break, seed baseline and Keeper failure
  containment are retained. Historical prototype policies in old planning docs
  are not the current authorization or gate policy.
