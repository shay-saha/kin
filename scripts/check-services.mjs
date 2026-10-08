import nextEnv from "@next/env";
import { createClient } from "@supabase/supabase-js";
import { readFileSync, existsSync } from "node:fs";
nextEnv.loadEnvConfig(process.cwd());
const names = ["NEXT_PUBLIC_SUPABASE_URL", "NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY", "SUPABASE_SECRET_KEY",
  "OPENAI_API_KEY", "DEEPGRAM_API_KEY", "ELEVENLABS_API_KEY", "ELEVENLABS_VOICE_ID",
  "KIN_FACE_TOKEN_KEY"];
const aliases = { NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY: "NEXT_PUBLIC_SUPABASE_ANON_KEY", SUPABASE_SECRET_KEY: "SUPABASE_SERVICE_ROLE_KEY" };
console.log(JSON.stringify({ missing: names.filter(n => !process.env[n] && !process.env[aliases[n]]) }));
const url = process.env.NEXT_PUBLIC_SUPABASE_URL;
const secret = process.env.SUPABASE_SECRET_KEY || process.env.SUPABASE_SERVICE_ROLE_KEY;
const publishable = process.env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY || process.env.NEXT_PUBLIC_SUPABASE_ANON_KEY;
const options = { auth: { persistSession: false, autoRefreshToken: false }, global: {
  fetch: (input, init) => fetch(input, { ...init, signal: AbortSignal.timeout(10000) }),
} };
async function main() {
  if (!url || !secret || !publishable) throw new Error("configuration");
  const service = createClient(url, secret, options);
  const auth = await service.auth.admin.listUsers({ page: 1, perPage: 100 });
  console.log(JSON.stringify({ auth: auth.error ? "FAIL" : "PASS", demoUsers: auth.data?.users?.filter(u => u.app_metadata.kin_family_id).length ?? 0 }));
  let schemaReady = true;
  let initialSchemaReady = false;
  for (const table of ["relatives", "memories", "graph_nodes", "graph_edges", "provenance", "face_embeddings", "recall_events", "weaver_questions", "ingestion_receipts", "wearer_accounts", "pending_contributions"]) {
    const result = await service.from(table).select(table === "relatives" ? "id,is_self,self_capture_open" : table === "wearer_accounts" ? "user_id,family_id" : table === "memories" ? "id,source,verified_facts" : table === "recall_events" ? "id,face_outcome,evidence,reason_code" : "id", { count: "exact" }).limit(1);
    schemaReady &&= !result.error && Array.isArray(result.data);
    if (table === "relatives") initialSchemaReady = !result.error && Array.isArray(result.data);
    console.log(JSON.stringify({ table, status: result.status, code: result.error?.code, count: result.count }));
  }
  const legacy = await service.from("wearer").select("family_id").eq("family_id", "demo").maybeSingle();
  schemaReady &&= !legacy.error && !legacy.data;
  const credentialPath = ".env.demo.local";
  const password = process.env.KIN_DEMO_PASSWORD || (existsSync(credentialPath)
    ? readFileSync(credentialPath, "utf8").match(/^KIN_DEMO_PASSWORD=(.+)$/m)?.[1]?.trim() : undefined);
  if (password && schemaReady) {
    for (const name of ["maya", "david", "elena", "rosa"]) {
      const client = createClient(url, publishable, options);
      const login = await client.auth.signInWithPassword({ email: name + "@demo.kin.test", password });
      console.log(JSON.stringify({ account: name, login: !login.error ? "PASS" : "FAIL", claims: Boolean(login.data.user?.app_metadata.kin_family_id && (name === "rosa" ? login.data.user?.app_metadata.kin_role === "wearer" && !login.data.user?.app_metadata.kin_contributor_id : login.data.user?.app_metadata.kin_contributor_id)) }));
      if (!login.error && schemaReady && name === "maya") {
        const family = login.data.user.app_metadata.kin_family_id;
        const rows = await client.from("relatives").select("id").eq("family_id", family);
        console.log(JSON.stringify({ authenticatedFamilyRead: rows.error ? "FAIL" : "PASS", rows: rows.data?.length }));
        const channel = client.channel("kin-live-check-" + Date.now()).on("postgres_changes",
          { event: "*", schema: "public", table: "recall_events", filter: "family_id=eq." + family }, () => {});
        const state = await new Promise(resolve => {
          const timeout = setTimeout(() => resolve("TIMEOUT"), 10000);
          channel.subscribe(status => { if (status === "SUBSCRIBED" || status === "CHANNEL_ERROR") { clearTimeout(timeout); resolve(status); } });
        });
        console.log(JSON.stringify({ realtimeSubscription: state }));
        await client.removeChannel(channel);
      }
      await client.auth.signOut();
    }
  }
  if (!schemaReady) console.log(JSON.stringify({ schema: "BLOCKED", action: initialSchemaReady ? "Initial schema exists. Apply any missing migrations through 011 in order; 006 is explicit_api_grants and 007 is self_contribution. Preserve existing family data." : "Apply migrations 001 through 011 in order, then provision and seed." }));
  if (process.env.KIN_FACE_SERVICE_URL && process.env.KIN_FACE_SERVICE_TOKEN) {
    const res = await fetch(process.env.KIN_FACE_SERVICE_URL, { method: "POST", headers: {
      authorization: "Bearer " + process.env.KIN_FACE_SERVICE_TOKEN, "content-type": "image/jpeg",
    }, body: new Uint8Array([0]), signal: AbortSignal.timeout(10000) });
    console.log(JSON.stringify({ faceAuthenticatedBoundary: res.status === 422 ? "PASS" : "FAIL", status: res.status, actualInference: "NOT_TESTED_NO_FIXTURE" }));
  }
}
// This is a one-shot CLI. Realtime/provider clients can retain background handles
// after all awaited checks finish; do not leave the terminal running indefinitely.
main().then(() => { process.exit(0); }).catch(() => {
  console.error("Service check failed (no provider details logged)."); process.exit(1);
});
