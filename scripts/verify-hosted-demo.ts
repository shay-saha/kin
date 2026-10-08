import { loadEnvConfig } from '@next/env';
import { createClient } from '@supabase/supabase-js';
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { DEMO_ACCOUNTS, DEMO_FAMILY_ID } from '../lib/demo';
loadEnvConfig(process.cwd());
let stage = 'configuration';
async function main() {
  const backend = (process.env.KIN_BACKEND_URL || 'http://127.0.0.1:8787').replace(/\/$/, '');
  const api = async (path: string, token: string, method = 'GET') => { const response = await fetch(backend + '/api/' + path, {method,headers:{authorization:'Bearer ' + token}}); return {response, data: await response.json()}; };
  const url = process.env.NEXT_PUBLIC_SUPABASE_URL!;
  const key = process.env.SUPABASE_SECRET_KEY ?? process.env.SUPABASE_SERVICE_ROLE_KEY!;
  const publicKey = process.env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY ?? process.env.NEXT_PUBLIC_SUPABASE_ANON_KEY!;
  const password = process.env.KIN_DEMO_PASSWORD ?? readFileSync('.env.demo.local','utf8').match(/^KIN_DEMO_PASSWORD=(.+)$/m)?.[1]?.trim().replace(/^(["'])(.*)\1$/, '$2');
  assert.ok(password);
  const options = { auth: { persistSession: false, autoRefreshToken: false } };
  const service = createClient(url,key,options);
  const expected = JSON.parse(readFileSync('brain/fixtures/demo.json', 'utf8'));
  stage = 'self Keeper';
  const self = await service.from('relatives').select('*').eq('family_id',DEMO_FAMILY_ID).eq('is_self',true).single();
  assert.equal(self.error,null);
  assert.equal(self.data.name,'Rosa');
  const expectedTables = { relatives: [...expected.relatives,self.data], memories: expected.memories, graph_nodes: expected.nodes, graph_edges: expected.edges, provenance: expected.provenance };
  stage = 'canonical content';
  for (const [table, rows] of Object.entries(expectedTables) as [string, {id: string}[]][]) {
    const result = await service.from(table).select('*');
    assert.equal(result.error,null);
    assert.deepEqual(result.data!.map(r=>r.id).sort(),rows.map((r: {id: string})=>r.id).sort());
    if (table !== 'provenance') assert.ok(result.data!.every(r=>r.family_id===DEMO_FAMILY_ID));
    if (table === 'memories') for (const memory of expected.memories) {
      const actual: Record<string, unknown> = result.data!.find(r=>r.id===memory.id)!;
      for (const field of ['transcript','summary','caption','contributor_id','kind'] as const) assert.equal(actual[field],memory[field]);
      assert.deepEqual(actual.verified_facts,memory.verified_facts);
      assert.equal((actual.source as { embedding_status: string }).embedding_status,'ready');
    }
    console.log(table+': canonical content verified ('+rows.length+')');
  }
  stage = 'accounts';
  const users=await service.auth.admin.listUsers({perPage:100});
  assert.equal(users.error,null);assert.equal(users.data.users.length,4);
  assert.deepEqual(users.data.users.map(u=>u.email).sort(),DEMO_ACCOUNTS.map(a=>a.email).sort());
  for (const account of DEMO_ACCOUNTS) {
    stage = account.name+' authorization';
    const client=createClient(url,publicKey,options);
    const login=await client.auth.signInWithPassword({email:account.email,password});
    assert.equal(login.error,null);
    const token=login.data.session!.access_token;
    const {response:familyResponse,data:identity}=await api('family',token);
    assert.equal(familyResponse.status,200);
    assert.equal(identity.familyId,DEMO_FAMILY_ID);
    assert.equal(login.data.user!.app_metadata.kin_contributor_id ?? null,account.contributorId);
    assert.equal(identity.relativeId,account.role==='wearer' ? self.data.id : account.contributorId);
    assert.equal(identity.isOwner,account.role==='organizer');
    if(account.role==='wearer') assert.equal((await api('self',token)).response.status,200);
    const pending = await client.from('pending_contributions').select('id');
    assert.equal(pending.error,null);
    if(account.role==='wearer') assert.equal(pending.data!.length,0);
    for (const [name,args] of [
      ['capture_self_contribution',{payload:{},preview:''}],
      ['review_self_contribution',{family:DEMO_FAMILY_ID,reviewer:self.data.id,contribution:self.data.id,decision:'approve'}],
    ] as const) assert.equal((await client.rpc(name,args)).error?.code,'42501');
    for(const [table,rows] of Object.entries(expectedTables) as [string, {id: string}[]][]) {
      const visible=await client.from(table).select('id');assert.equal(visible.error,null);assert.equal(visible.data!.length,rows.length);
      if(table!=='provenance') {const foreign=await client.from(table).select('id').neq('family_id',DEMO_FAMILY_ID);assert.equal(foreign.error,null);assert.equal(foreign.data!.length,0);}
    }
    for(const table of ['face_embeddings','ingestion_receipts']) {const restricted=await client.from(table).select('id');assert.equal(restricted.error?.code,'42501');}
    const rpc=await client.rpc('match_subject_memories',{query:Array(1536).fill(0),family:DEMO_FAMILY_ID,contributor:DEMO_ACCOUNTS[0].contributorId,subject:expected.nodes[0].id,k:1});
    assert.equal(rpc.error?.code,'42501');
    await client.auth.signOut();
    console.log(account.name+': sign-in, family reads, role boundaries and privileged-access denial verified');
  }
  stage='anonymous access';
  const anon=createClient(url,publicKey,options);
  for(const table of Object.keys(expectedTables)) {const result=await anon.from(table).select('id');assert.ok(result.error?.code==='42501'||(!result.error&&result.data?.length===0));}
  console.log('Anonymous family data: inaccessible');

}
main().catch(()=>{console.error(JSON.stringify({verification:'FAILED',stage}));process.exitCode=1;});
