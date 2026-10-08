// Runs the unmodified production migrations on PostgreSQL WASM with real pgvector.
// Only Supabase-owned auth/storage schemas and roles are represented by local stubs.
import { PGlite } from '@electric-sql/pglite';
import { vector } from '@electric-sql/pglite-pgvector';
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

const db = await PGlite.create({ extensions: { vector } });
let count = 0;
const uuid = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;
const embedding = Array(1536).fill(0.01);
const faceModel = 'face-api-1.7.15:ssd-mobilenetv1:landmark68:recognition128:rgb-exif-v1';
async function test(name, fn) { await fn(); console.log(`PASS ${name}`); count++; }
async function scalar(sql, params = []) { return (await db.query(sql, params)).rows[0].value; }
async function rejectCode(fn, code) { await assert.rejects(fn, (e) => e.code === code); }
async function commit(payload) { return scalar('select public.commit_ingestion($1::jsonb) as value', [JSON.stringify(payload)]); }
function memoryPayload(id, kind = 'story', transcript = 'Nora likes lemon cake.') {
  return { id: uuid(id), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', contributor_id: uuid(1), request_hash: `hash-${id}`,
    source: { type: 'human', caption: '' }, response: { memory_id: uuid(id) },
    memory: { id: uuid(id), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', contributor_id: uuid(1), kind, transcript,
      summary: transcript, embedding, verified_facts: [{ id: uuid(id + 1000), subjectNodeId: uuid(10),
        text: transcript, sourceSpan: { start: 0, end: transcript.length }, memoryId: uuid(id), contributorId: uuid(1) }] },
    nodes: [], edges: [], provenance: [{ id: uuid(id + 2000), memory_id: uuid(id), contributor_id: uuid(1), node_id: uuid(10), edge_id: null }] };
}
try {
  await db.exec(`
    create role anon; create role authenticated; create role service_role bypassrls;
    create schema auth; create schema storage; create schema extensions;
    create table auth.users(id uuid primary key, raw_app_meta_data jsonb default '{}'::jsonb);
    create function auth.jwt() returns jsonb language sql stable as $$
      select coalesce(nullif(current_setting('request.jwt.claims', true), ''), '{}')::jsonb $$;
    create function auth.uid() returns uuid language sql stable as $$ select (auth.jwt()->>'sub')::uuid $$;
    create table storage.buckets(id text primary key, name text, public boolean);
    create table storage.objects(id uuid primary key default gen_random_uuid(), bucket_id text, name text);
    alter table storage.objects enable row level security;
    create function storage.foldername(name text) returns text[] language sql immutable as $$
      select (string_to_array(name, '/'))[1:array_length(string_to_array(name, '/'), 1)-1] $$;
    create publication supabase_realtime;
  `);
  await test('all numbered migrations apply with real pgvector', async () => {
    for (const name of ['001_init.sql', '002_atomic_ingestion.sql', '003_family_boundary_and_weaver.sql', '004_wearer_membership.sql', '005_consolidate_hackmit_demo.sql', '006_explicit_api_grants.sql', '007_self_contribution.sql', '008_living_stories.sql', '009_family_accounts.sql', '010_loved_one_invites.sql', '011_rust_face_models.sql']) {
      await db.exec(await readFile(new URL(`../../supabase/migrations/${name}`, import.meta.url), 'utf8'));
    }
  });
  await test('timing trigger preserves atomic row inserts and legacy recordings', async () => {
    await db.exec('begin');
    try {
      const id = uuid(990001);
      const contributor = { id: uuid(990000), family_id: 'timing-test' };
      await db.query("insert into public.relatives(id, family_id, name, relation_to_wearer, color) values ($1, $2, 'Maya', 'granddaughter', '#355746')", [contributor.id, contributor.family_id]);
      const segments = [{ start: 0, end: 1, text: 'Hello.' }];
      await db.query("insert into public.memories(id, family_id, contributor_id, kind, summary, source, audio_segments, embedding) values ($1, $2, $3, 'story', 'Hello.', $4, null, $5::vector)", [id, contributor.family_id, contributor.id, JSON.stringify({ type: 'human', audio_segments: segments }), JSON.stringify(embedding)]);
      assert.deepEqual(await scalar('select audio_segments as value from public.memories where id=$1', [id]), segments);
      await db.query("insert into public.memories(id, family_id, contributor_id, kind, summary, source, audio_segments, embedding) values ($1, $2, $3, 'story', 'Legacy.', $4, null, $5::vector)", [uuid(990002), contributor.family_id, contributor.id, JSON.stringify({ type: 'human' }), JSON.stringify(embedding)]);
      assert.deepEqual(await scalar('select audio_segments as value from public.memories where id=$1', [uuid(990002)]), []);
    } finally { await db.exec('rollback'); }
  });
  await test('consolidation preserves both datasets, deduplicates graph IDs and is repeatable', async () => {
    const baseline = JSON.parse(await readFile(new URL('../../demo/shared-baseline.json', import.meta.url), 'utf8'));
    const sql = await readFile(new URL('../../supabase/migrations/005_consolidate_hackmit_demo.sql', import.meta.url), 'utf8');
    const pairs = [...sql.matchAll(/\('([0-9a-f-]{36})','([0-9a-f-]{36})'\)/g)].map(m => [m[1], m[2]]);
    const reverse = value => pairs.find(p => p[1] === value)?.[0] ?? value;
    const family = '670f5075-c286-4b29-8074-86401c18d0c0';
    const legacyMemories = new Set(baseline.memories.slice(-3).map(m => m.id));
    async function insert(table, row) {
      const entries = Object.entries(row);
      await db.query('insert into '+table+'('+entries.map(([k])=>k).join(',')+') values ('+entries.map((_,i)=>'$'+(i+1)).join(',')+')', entries.map(([,v])=>v));
    }
    for (const table of ['relatives','graph_nodes','graph_edges']) {
      for (const row of baseline[table]) {
        await insert(table, row);
        if (reverse(row.id) !== row.id) await insert(table, { ...row, id: reverse(row.id), family_id: 'demo',
          ...(table==='graph_edges' ? { from_node: reverse(row.from_node), to_node: reverse(row.to_node) } : {}) });
      }
    }
    await db.exec("insert into wearer values('demo','Rosa'),('"+family+"','Rosa')");
    for (const row of baseline.memories) await insert('memories', { ...row, embedding: JSON.stringify(embedding),
      family_id: legacyMemories.has(row.id) ? 'demo' : family,
      contributor_id: legacyMemories.has(row.id) ? reverse(row.contributor_id) : row.contributor_id });
    for (const row of baseline.provenance) await insert('provenance', legacyMemories.has(row.memory_id) ? { ...row,
      contributor_id: reverse(row.contributor_id), node_id: reverse(row.node_id), edge_id: reverse(row.edge_id) } : row);
    const cake = baseline.graph_nodes.find(n => n.label === 'Sunday lemon cake').id;
    for (const [index, scope] of [family,'demo'].entries()) await insert('weaver_questions', {
      id: uuid(9100+index), family_id: scope, target_relative_id: scope === 'demo' ? reverse(baseline.relatives[0].id) : baseline.relatives[0].id,
      gap_node_id: scope === 'demo' ? reverse(cake) : cake, gap_type: 'missing_origin', question_text: 'Where did the recipe come from?', evidence: '[]', status: 'open',
    });
    for (let attempt=0; attempt<2; attempt++) {
      await db.exec(sql);
      for (const table of ['relatives','graph_nodes','graph_edges','memories','provenance']) assert.equal(await scalar('select count(*)::int as value from '+table), baseline[table].length);
      assert.equal(await scalar("select count(*)::int as value from wearer where family_id='demo'"),0);
      assert.equal(await scalar("select count(*)::int as value from weaver_questions where status='open'"),1);
      assert.equal(await scalar("select count(*)::int as value from weaver_questions where status='superseded'"),1);
    }
    for (const table of ['weaver_questions','memories','graph_edges','graph_nodes','relatives','wearer']) await db.exec('delete from '+table);
  });
  await db.exec(`grant usage on schema public, auth, storage to anon, authenticated, service_role;
    grant select on all tables in schema public to anon, authenticated;
    revoke all on public.face_embeddings, public.ingestion_receipts from anon, authenticated;
    grant all on all tables in schema public to service_role;
    grant select on storage.objects to authenticated;
    insert into relatives(id,family_id,name,relation_to_wearer,color) values
      ('${uuid(1)}','670f5075-c286-4b29-8074-86401c18d0c0','David','son','#ffffff'), ('${uuid(2)}','other','Other','son','#ffffff');
    insert into graph_nodes(id,family_id,type,label) values
      ('${uuid(10)}','670f5075-c286-4b29-8074-86401c18d0c0','person','Nora'), ('${uuid(11)}','other','person','Other'),
      ('${uuid(12)}','670f5075-c286-4b29-8074-86401c18d0c0','tradition','Sunday lemon cake baking');`);
  await test('one memory and receipt across same retry; changed retry conflicts', async () => {
    const p = memoryPayload(100);
    assert.deepEqual(await commit(p), p.response);
    assert.deepEqual(await commit(p), p.response);
    assert.equal(await scalar('select count(*)::int as value from memories'), 1);
    await rejectCode(() => commit({ ...p, request_hash: 'changed' }), '23505');
  });
  await test('graph error rolls the memory and receipt back', async () => {
    const p = memoryPayload(101);
    p.edges = [{ id: uuid(501), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', from_node: uuid(10), to_node: uuid(11), rel: 'friend_of' }];
    await rejectCode(() => commit(p), 'P0001');
    assert.equal(await scalar('select count(*)::int as value from memories where id=$1', [p.id]), 0);
    assert.equal(await scalar('select count(*)::int as value from ingestion_receipts where id=$1', [p.id]), 0);
  });
  await test('model-generated fact text cannot masquerade as a human span', async () => {
    const p = memoryPayload(102);
    p.memory.verified_facts[0].text = 'Nora founded a company.';
    await rejectCode(() => commit(p), 'P0001');
    assert.equal(await scalar('select count(*)::int as value from memories where id=$1', [p.id]), 0);
  });
  await test('contributor and family scope mismatch rolls back', async () => {
    const p = memoryPayload(103);
    p.contributor_id = uuid(2);
    await rejectCode(() => commit(p), 'P0001');
  });
  await db.exec(`insert into weaver_questions(id,family_id,target_relative_id,gap_node_id,gap_type,question_text,evidence)
    values ('${uuid(30)}','670f5075-c286-4b29-8074-86401c18d0c0','${uuid(1)}','${uuid(12)}','missing_origin','Where did it come from?','[]');`);
  await test('one open question per family gap', async () => {
    await rejectCode(() => db.exec(`insert into weaver_questions(family_id,target_relative_id,gap_node_id,gap_type,question_text,evidence)
      values ('670f5075-c286-4b29-8074-86401c18d0c0','${uuid(1)}','${uuid(12)}','missing_origin','Duplicate','[]')`), '23505');
  });
  await test('failed answer keeps question open', async () => {
    const p = memoryPayload(104, 'answer'); p.memory.source_question_id = uuid(30);
    p.provenance[0].contributor_id = uuid(2);
    await rejectCode(() => commit(p), 'P0001');
    assert.equal(await scalar('select status as value from weaver_questions where id=$1', [uuid(30)]), 'open');
  });
  await test('answer, graph origin, provenance and question complete atomically and retry once', async () => {
    const p = memoryPayload(105, 'answer', "It was actually their mother's recipe. She brought it from Italy.");
    p.memory.source_question_id = uuid(30);
    p.nodes = [{ id: uuid(13), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', type: 'place', label: 'Italy', aliases: [] }];
    p.edges = [{ id: uuid(500), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', from_node: uuid(12), to_node: uuid(13), rel: 'origin' }];
    p.provenance.push({ id: uuid(600), memory_id: p.id, contributor_id: uuid(1), node_id: null, edge_id: uuid(500) });
    await commit(p); await commit(p);
    assert.equal(await scalar('select status as value from weaver_questions where id=$1', [uuid(30)]), 'answered');
    assert.equal(await scalar('select count(*)::int as value from memories where source_question_id=$1', [uuid(30)]), 1);
    assert.equal(await scalar("select count(*)::int as value from graph_edges where from_node=$1 and rel='origin'", [uuid(12)]), 1);
    const search = await db.query('select * from match_subject_memories($1::vector,$2,$3,$4,10)', [JSON.stringify(embedding),'670f5075-c286-4b29-8074-86401c18d0c0',uuid(1),uuid(10)]);
    assert.ok(search.rows.some((r) => r.id === p.id));
    assert.equal((await db.query('select * from match_subject_memories($1::vector,$2,$3,$4,10)', [JSON.stringify(embedding),'other',uuid(1),uuid(10)])).rows.length, 0);
    const duplicate = memoryPayload(106, 'answer'); duplicate.memory.source_question_id = uuid(30);
    await rejectCode(() => commit(duplicate), '23505');
  });
  await test('face enrollment requires consent and stores canonical model; old rows remain ineligible', async () => {
    await commit(memoryPayload(110, 'photo'));
    const p = { id: uuid(111), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', contributor_id: uuid(1), request_hash: 'face', response: { ok: true },
      source: { type: 'human', consent: false, model: faceModel },
      face: { id: uuid(111), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', contributor_id: uuid(1), person_node_id: uuid(10), memory_id: uuid(110), descriptor: Array(128).fill(.01) } };
    await rejectCode(() => commit(p), 'P0001');
    p.source.consent = true; await commit(p); await commit(p);
    assert.equal(await scalar('select model as value from face_embeddings where id=$1', [p.id]), faceModel);
    await db.query('insert into face_embeddings(id,family_id,contributor_id,person_node_id,memory_id,descriptor) values($1,$2,$3,$4,$5,$6::vector)',
      [uuid(112),'670f5075-c286-4b29-8074-86401c18d0c0',uuid(1),uuid(10),uuid(110),JSON.stringify(Array(128).fill(.01))]);
    assert.equal(await scalar('select count(*)::int as value from face_embeddings where model=$1', [faceModel]), 1);
  });
  await test('Rust face-model migration accepts native descriptors, rejects unknown models, and is repeatable', async () => {
    const nativeModel = 'kin-yunet-2023mar:sface-2021dec:recognition128:rgb-exif-v1';
    const payload = { id: uuid(113), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', contributor_id: uuid(1), request_hash: 'rust-face', response: { ok: true },
      source: { type: 'human', consent: true, model: nativeModel },
      face: { id: uuid(113), family_id: '670f5075-c286-4b29-8074-86401c18d0c0', contributor_id: uuid(1), person_node_id: uuid(10), memory_id: uuid(110), descriptor: Array(128).fill(.01) } };
    await commit(payload); await commit(payload);
    assert.equal(await scalar('select model as value from face_embeddings where id=$1', [payload.id]), nativeModel);
    const unknown = structuredClone(payload); unknown.id=uuid(114); unknown.face.id=uuid(114); unknown.source.model='untrusted-model';
    await rejectCode(() => commit(unknown), 'P0001');
    await db.exec(await readFile(new URL('../../supabase/migrations/011_rust_face_models.sql', import.meta.url), 'utf8'));
    assert.equal(await scalar('select model as value from face_embeddings where id=$1', [uuid(111)]), faceModel);
    assert.equal(await scalar('select model as value from face_embeddings where id=$1', [payload.id]), nativeModel);
  });
  await test('authenticated family reads isolate other families and reject privileged RPC/descriptors', async () => {
    await db.exec(`set role authenticated; set request.jwt.claims = '{"app_metadata":{"kin_family_id":"other","kin_contributor_id":"${uuid(2)}"}}';`);
    assert.equal(await scalar('select count(*)::int as value from memories'), 0);
    assert.equal(await scalar('select count(*)::int as value from graph_nodes'), 1);
    for (const table of ['relatives','wearer','memories','graph_nodes','graph_edges','recall_events','weaver_questions']) {
      assert.equal(await scalar(`select count(*)::int as value from ${table} where family_id <> 'other'`), 0);
    }
    assert.equal(await scalar('select count(*)::int as value from provenance'), 0);
    await rejectCode(() => commit(memoryPayload(120)), '42501');
    await rejectCode(() => db.exec('select * from face_embeddings'), '42501');
    await db.exec('reset role');
  });
  await test('family claim without real matching contributor membership cannot read', async () => {
    await db.exec(`set role authenticated; set request.jwt.claims = '{"app_metadata":{"kin_family_id":"670f5075-c286-4b29-8074-86401c18d0c0","kin_contributor_id":"${uuid(2)}"}}';`);
    for (const table of ['relatives','wearer','memories','graph_nodes','graph_edges','provenance','recall_events','weaver_questions']) {
      assert.equal(await scalar(`select count(*)::int as value from ${table}`), 0);
    }
    await db.exec('reset role');
  });
  await test('anonymous reads reveal no family records', async () => {
    await db.exec('set role anon');
    for (const table of ['relatives','wearer','memories','graph_nodes','graph_edges','provenance','recall_events','weaver_questions']) {
      assert.equal(await scalar(`select count(*)::int as value from ${table}`), 0);
    }
    await db.exec('reset role');
  });
  await test('private storage authorizes only actual matching family members', async () => {
    assert.equal(await scalar("select public as value from storage.buckets where id='media'"), false);
    await db.exec("insert into storage.objects(bucket_id,name) values('media','670f5075-c286-4b29-8074-86401c18d0c0/a/b/image'),('media','other/a/b/image')");
    await db.exec(`set role authenticated; set request.jwt.claims = '{"app_metadata":{"kin_family_id":"670f5075-c286-4b29-8074-86401c18d0c0","kin_contributor_id":"${uuid(1)}"}}';`);
    assert.equal(await scalar('select count(*)::int as value from storage.objects'), 1);
    await db.exec('reset role');
  });
  await test('wearer membership uses auth user and family, never a fake contributor', async () => {
    await db.exec("insert into wearer(family_id,name) values('670f5075-c286-4b29-8074-86401c18d0c0','Rosa'); insert into auth.users(id) values('"+uuid(900)+"'); insert into wearer_accounts values('"+uuid(900)+"','670f5075-c286-4b29-8074-86401c18d0c0');");
    await db.exec(`set role authenticated; set request.jwt.claims = '{"sub":"${uuid(900)}","app_metadata":{"kin_family_id":"670f5075-c286-4b29-8074-86401c18d0c0","kin_role":"wearer"}}';`);
    assert.equal(await scalar('select count(*)::int as value from wearer'), 1);
    assert.equal(await scalar("select count(*)::int as value from graph_nodes where family_id='other'"), 0);
    await rejectCode(() => commit(memoryPayload(902)), '42501');
    await rejectCode(() => db.exec('select * from face_embeddings'), '42501');
    await db.exec(`set request.jwt.claims = '{"sub":"${uuid(901)}","app_metadata":{"kin_family_id":"670f5075-c286-4b29-8074-86401c18d0c0","kin_role":"wearer"}}';`);
    assert.equal(await scalar('select count(*)::int as value from wearer'), 0);
    await db.exec(`set request.jwt.claims = '{"sub":"${uuid(900)}","app_metadata":{"kin_family_id":"other","kin_role":"wearer"}}';`);
    assert.equal(await scalar('select count(*)::int as value from graph_nodes'), 0);
    await db.exec('reset role');
  });
  const family = '670f5075-c286-4b29-8074-86401c18d0c0';
  const selfId = uuid(3);
  await db.query('insert into relatives(id,family_id,name,relation_to_wearer,color,is_self,self_capture_open) values($1,$2,$3,$4,$5,true,false)', [selfId,family,'Rosa','self','#ffffff']);
  const selfPayload = id => {
    const p = memoryPayload(id);
    p.contributor_id = selfId; p.memory.contributor_id = selfId;
    p.memory.verified_facts.forEach(f => f.contributorId = selfId);
    p.provenance.forEach(f => f.contributor_id = selfId);
    return p;
  };
  const capture = p => scalar('select capture_self_contribution($1::jsonb,$2) as value',[JSON.stringify(p),'Nora likes lemon cake.']);
  const review = (id,decision,reviewer=uuid(1),scope=family) => scalar('select review_self_contribution($1,$2,$3,$4) as value',[scope,reviewer,uuid(id),decision]);
  await test('closed capture is immutable, rejects changed retries and never revives a rejected story', async () => {
    const p = selfPayload(950);
    assert.equal((await capture(p)).pending_review,true);
    assert.equal((await capture(p)).pending_review,true);
    await rejectCode(()=>capture({...p,request_hash:'changed'}),'23505');
    assert.deepEqual(await scalar('select payload as value from pending_contributions where id=$1',[p.id]),p);
    await review(950,'reject');
    await review(950,'reject');
    await rejectCode(()=>capture(p),'23505');
    await rejectCode(()=>review(950,'approve'),'23505');
    assert.equal(await scalar('select count(*)::int as value from memories where id=$1',[p.id]),0);
  });
  await test('approval publishes once; a competing rejection cannot overwrite it', async () => {
    const p=selfPayload(951); await capture(p);
    assert.equal((await review(951,'approve')).state,'approved');
    assert.deepEqual(await review(951,'approve'),await review(951,'approve'));
    await rejectCode(()=>review(951,'reject'),'23505');
    assert.equal((await capture(p)).pending_review,false);
    assert.equal(await scalar('select count(*)::int as value from memories where id=$1',[p.id]),1);
    assert.equal(await scalar('select count(*)::int as value from ingestion_receipts where id=$1',[p.id]),1);
  });
  await test('failed approval rolls back publication and preserves the pending decision', async () => {
    const p=selfPayload(952); p.edges=[{id:uuid(953),family_id:family,from_node:uuid(10),to_node:uuid(11),rel:'friend_of'}];
    await capture(p); await rejectCode(()=>review(952,'approve'),'P0001');
    assert.equal(await scalar('select state as value from pending_contributions where id=$1',[p.id]),'pending');
    assert.equal(await scalar('select count(*)::int as value from memories where id=$1',[p.id]),0);
    assert.equal(await scalar('select count(*)::int as value from ingestion_receipts where id=$1',[p.id]),0);
  });
  await test('capture reads the current window and cannot bypass a queued decision after reopening', async () => {
    const p=selfPayload(954); await capture(p);
    await db.query('update relatives set self_capture_open=true where id=$1',[selfId]);
    assert.equal((await capture(p)).pending_review,true);
    const direct=selfPayload(955); assert.deepEqual(await capture(direct),direct.response);
    await db.query('update relatives set self_capture_open=false where id=$1',[selfId]);
    assert.deepEqual(await capture(direct),direct.response);
    assert.equal((await capture(selfPayload(956))).pending_review,true);
  });
  await test('self review rejects wearer and foreign contributors; RLS hides queue from wearer and foreign family', async () => {
    await rejectCode(()=>review(956,'approve',selfId),'42501');
    await rejectCode(()=>review(956,'approve',uuid(2)),'42501');
    await rejectCode(()=>review(956,'approve',uuid(2),'other'),'P0002');
    for (const metadata of [
      {sub:uuid(900),app_metadata:{kin_family_id:family,kin_role:'wearer'}},
      {app_metadata:{kin_family_id:'other',kin_contributor_id:uuid(2)}},
      {app_metadata:{kin_family_id:family,kin_contributor_id:uuid(1)}},
    ]) {
      await db.exec('set role authenticated');
      await db.query("select set_config('request.jwt.claims',$1,false)",[JSON.stringify(metadata)]);
      const visible=await scalar('select count(*)::int as value from pending_contributions');
      assert.equal(visible,metadata.app_metadata.kin_contributor_id===uuid(1)?5:0);
      await rejectCode(()=>capture(selfPayload(957)),'42501');
      await rejectCode(()=>review(956,'approve'),'42501');
      await db.exec('reset role');
    }
  });
  await test('reapplying 007 preserves queued decisions, memories and the current window', async () => {
    const before = await db.query('select * from pending_contributions order by id');
    const memories = await scalar('select count(*)::int as value from memories');
    await db.exec(await readFile(new URL('../../supabase/migrations/007_self_contribution.sql',import.meta.url),'utf8'));
    assert.deepEqual((await db.query('select * from pending_contributions order by id')).rows,before.rows);
    assert.equal(await scalar('select count(*)::int as value from memories'),memories);
    assert.equal(await scalar('select self_capture_open as value from relatives where id=$1',[selfId]),false);
  });
  await test('reset deletion order clears answers, pending stories, receipts and graph while retaining memberships', async () => {
    for (const table of ['pending_contributions','weaver_questions','recall_events','ingestion_receipts','face_embeddings','memories','graph_edges','graph_nodes']) {
      await db.query(`delete from ${table} where family_id = $1`, ['670f5075-c286-4b29-8074-86401c18d0c0']);
    }
    assert.equal(await scalar('select count(*)::int as value from provenance'), 0);
    await db.query('update relatives set self_capture_open=true where family_id=$1 and is_self',[family]);
    assert.equal(await scalar('select count(*)::int as value from pending_contributions'),0);
    assert.equal(await scalar('select self_capture_open as value from relatives where id=$1',[selfId]),true);
    assert.equal(await scalar("select count(*)::int as value from relatives where family_id='670f5075-c286-4b29-8074-86401c18d0c0'"), 2);
    assert.equal(await scalar('select count(*)::int as value from ingestion_receipts'), 0);
  });

  await test('new accounts create private families without exposing face descriptors', async () => {
    await db.query('insert into auth.users(id) values($1),($2),($3)',[uuid(9901),uuid(9902),uuid(9903)]);
    await db.exec('set role authenticated');
    await db.query("select set_config('request.jwt.claims',$1,false)",[JSON.stringify({sub:uuid(9901)})]);
    const family=await scalar("select create_kin_family('Ada','Lee','child') as value");
    assert.equal(await scalar('select kin_family_id() as value'),family);
    assert.equal(await scalar('select count(*)::int as value from relatives'),1);
    await rejectCode(()=>scalar('select count(*) as value from face_embeddings'),'42501');
    await db.query("select set_config('request.jwt.claims',$1,false)",[JSON.stringify({sub:uuid(9902)})]);
    assert.equal(await scalar('select count(*)::int as value from relatives'),0);
    await db.exec('reset role');
    const owner=await scalar("select raw_app_meta_data->>'kin_family_id' as value from auth.users where id=$1",[uuid(9901)]);
    assert.equal(owner,family);
    await db.query("insert into family_invites(token,family_id,role) values($1,$2,'loved_one')",[uuid(9910),family]);
    await db.exec('set role authenticated');
    await db.query("select set_config('request.jwt.claims',$1,false)",[JSON.stringify({sub:uuid(9902)})]);
    assert.equal(await scalar("select join_kin_family($1,null,null) as value",[uuid(9910)]),family);
    await db.exec('reset role');
    assert.equal(await scalar('select count(*)::int as value from wearer_accounts where user_id=$1',[uuid(9902)]),1);
    assert.equal(await scalar('select count(*)::int as value from relatives where family_id=$1 and is_self',[family]),1);
    assert.equal(await scalar('select self_capture_open as value from relatives where family_id=$1 and is_self',[family]),false);
    await db.exec('set role authenticated');
    await db.query("select set_config('request.jwt.claims',$1,false)",[JSON.stringify({sub:uuid(9903)})]);
    await rejectCode(()=>scalar('select join_kin_family($1,null,null) as value',[uuid(9910)]),'P0001');
    await db.exec('reset role');
  });
  await test('account migrations preserve existing records when reapplied', async()=>{
    const memories=await scalar('select count(*)::int as value from memories');
    const members=await db.query('select * from family_members order by user_id');
    for(const name of ['009_family_accounts.sql','010_loved_one_invites.sql'])
      await db.exec(await readFile(new URL(`../../supabase/migrations/${name}`,import.meta.url),'utf8'));
    assert.equal(await scalar('select count(*)::int as value from memories'),memories);
    const after=(await db.query('select * from family_members order by user_id')).rows;
    for(const member of members.rows) assert.deepEqual(after.find(m=>m.user_id===member.user_id),member);
    await db.exec(await readFile(new URL('../../supabase/migrations/010_loved_one_invites.sql',import.meta.url),'utf8'));
    assert.deepEqual((await db.query('select * from family_members order by user_id')).rows,after);
  });
  console.log(`${count} database integration checks passed (local PostgreSQL WASM + real pgvector; not hosted Supabase).`);
} finally { await db.close(); }
