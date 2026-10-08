begin;

create or replace function public.commit_ingestion(payload jsonb)
returns jsonb
language plpgsql
set search_path = public, extensions
as $$
declare
  operation_id uuid := (payload->>'id')::uuid;
  family text := payload->>'family_id';
  contributor uuid := (payload->>'contributor_id')::uuid;
  receipt public.ingestion_receipts%rowtype;
  memory_record public.memories%rowtype;
  face_record public.face_embeddings%rowtype;
  question public.weaver_questions%rowtype;
  node_record public.graph_nodes%rowtype;
  edge_record public.graph_edges%rowtype;
  provenance_record public.provenance%rowtype;
  item jsonb;
begin
  if family is null or contributor is null or operation_id is null
     or payload->>'request_hash' is null or payload->'response' is null
     or payload->'source'->>'type' is distinct from 'human' then
    raise exception 'Invalid ingestion payload';
  end if;
  perform pg_advisory_xact_lock(hashtextextended(family, 0));
  if not exists (select 1 from public.relatives where id = contributor and family_id = family) then
    raise exception 'Contributor scope mismatch';
  end if;
  select * into receipt from public.ingestion_receipts where id = operation_id;
  if found then
    if receipt.family_id is distinct from family
       or receipt.contributor_id is distinct from contributor
       or receipt.request_hash is distinct from payload->>'request_hash' then
      raise exception 'Idempotency conflict' using errcode = '23505';
    end if;
    return receipt.response;
  end if;

  if payload ? 'face' then
    face_record := jsonb_populate_record(null::public.face_embeddings,
      (payload->'face') || jsonb_build_object('descriptor', (payload->'face'->'descriptor')::text,
        'model', payload->'source'->>'model'));
    if face_record.id is distinct from operation_id
       or face_record.family_id is distinct from family
       or face_record.contributor_id is distinct from contributor
       or payload->'source'->>'consent' is distinct from 'true'
       or coalesce(payload->'source'->>'model', '') not in ('face-api-1.7.15:ssd-mobilenetv1:landmark68:recognition128:rgb-exif-v1', 'kin-yunet-2023mar:sface-2021dec:recognition128:rgb-exif-v1')
       or vector_dims(face_record.descriptor) is distinct from 128 then
      raise exception 'Invalid enrollment';
    end if;
    if not exists (select 1 from public.memories where id = face_record.memory_id
      and family_id = family and contributor_id = contributor and kind = 'photo')
      or not exists (select 1 from public.graph_nodes where id = face_record.person_node_id
      and family_id = family and type = 'person') then
      raise exception 'Enrollment scope mismatch';
    end if;
    insert into public.face_embeddings select face_record.*;
    insert into public.provenance(memory_id, node_id, contributor_id)
      select face_record.memory_id, face_record.person_node_id, contributor
      where not exists (select 1 from public.provenance where memory_id = face_record.memory_id
        and node_id = face_record.person_node_id and contributor_id = contributor);
  else
    memory_record := jsonb_populate_record(null::public.memories,
      (payload->'memory') || jsonb_build_object('embedding', (payload->'memory'->'embedding')::text));
    memory_record.source := payload->'source';
    memory_record.verified_facts := coalesce(payload->'memory'->'verified_facts', '[]'::jsonb);
    if memory_record.id is distinct from operation_id
       or memory_record.family_id is distinct from family
       or memory_record.contributor_id is distinct from contributor then
      raise exception 'Memory scope mismatch';
    end if;
    if memory_record.kind = 'answer' then
      select * into question from public.weaver_questions
        where id = memory_record.source_question_id for update;
      if not found or question.family_id is distinct from family
         or question.target_relative_id is distinct from contributor then
        raise exception 'Question scope mismatch';
      end if;
      if question.status <> 'open' then
        raise exception 'Question already answered' using errcode = '23505';
      end if;
    elsif memory_record.source_question_id is not null then
      raise exception 'Unexpected question';
    end if;
    memory_record.created_at := now();
    insert into public.memories select memory_record.*;

    for item in select value from jsonb_array_elements(payload->'nodes') loop
      node_record := jsonb_populate_record(null::public.graph_nodes, item);
      if node_record.family_id is distinct from family then raise exception 'Node scope mismatch'; end if;
      node_record.created_at := now();
      insert into public.graph_nodes select node_record.* on conflict (id) do nothing;
      if not exists (select 1 from public.graph_nodes where id = node_record.id
        and family_id = family and type = node_record.type and label = node_record.label) then
        raise exception 'Node collision';
      end if;
    end loop;
    for item in select value from jsonb_array_elements(payload->'edges') loop
      edge_record := jsonb_populate_record(null::public.graph_edges, item);
      if edge_record.family_id is distinct from family
         or not exists (select 1 from public.graph_nodes where id = edge_record.from_node and family_id = family)
         or not exists (select 1 from public.graph_nodes where id = edge_record.to_node and family_id = family)
         or edge_record.rel not in ('sibling_of','child_of','parent_of','grandchild_of','spouse_of','friend_of',
           'participates_in','started_by','taught_by','origin','located_at','wears','owns','made','happens_on') then
        raise exception 'Invalid edge';
      end if;
      edge_record.created_at := now();
      insert into public.graph_edges select edge_record.* on conflict (id) do nothing;
      if not exists (select 1 from public.graph_edges where id = edge_record.id and family_id = family
        and from_node = edge_record.from_node and to_node = edge_record.to_node and rel = edge_record.rel) then
        raise exception 'Edge collision';
      end if;
    end loop;
    for item in select value from jsonb_array_elements(payload->'provenance') loop
      provenance_record := jsonb_populate_record(null::public.provenance, item);
      if provenance_record.memory_id is distinct from operation_id
         or provenance_record.contributor_id is distinct from contributor
         or (provenance_record.node_id is null) = (provenance_record.edge_id is null)
         or (provenance_record.node_id is not null and not exists
           (select 1 from public.graph_nodes where id = provenance_record.node_id and family_id = family))
         or (provenance_record.edge_id is not null and not exists
           (select 1 from public.graph_edges where id = provenance_record.edge_id and family_id = family)) then
        raise exception 'Provenance scope mismatch';
      end if;
      insert into public.provenance select provenance_record.* on conflict (id) do nothing;
    end loop;
    for item in select value from jsonb_array_elements(memory_record.verified_facts) loop
      if item->>'memoryId' is distinct from operation_id::text
         or item->>'contributorId' is distinct from contributor::text
         or nullif(item->>'text', '') is null
         or jsonb_typeof(item->'sourceSpan') is distinct from 'object'
         or jsonb_typeof(item->'sourceSpan'->'start') is distinct from 'number'
         or jsonb_typeof(item->'sourceSpan'->'end') is distinct from 'number'
         or (item->'sourceSpan'->>'start')::int < 0
         or (item->'sourceSpan'->>'end')::int <= (item->'sourceSpan'->>'start')::int
         or position(item->>'text' in coalesce(memory_record.transcript, memory_record.source->>'caption', '')) = 0
         or not exists (select 1 from public.provenance where memory_id = operation_id
           and node_id = (item->>'subjectNodeId')::uuid and contributor_id = contributor) then
        raise exception 'Invalid verified human fact';
      end if;
    end loop;
    if memory_record.kind = 'answer' then
      update public.weaver_questions set status = 'answered', answer_memory_id = operation_id
        where id = memory_record.source_question_id;
    end if;
  end if;

  insert into public.ingestion_receipts(id, family_id, contributor_id, request_hash, response, source)
    values (operation_id, family, contributor, payload->>'request_hash', payload->'response', payload->'source');
  return payload->'response';
end;
$$;

revoke all on function public.commit_ingestion(jsonb) from public, anon, authenticated;
grant execute on function public.commit_ingestion(jsonb) to service_role;

notify pgrst, 'reload schema';

commit;
