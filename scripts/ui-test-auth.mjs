// Disposable loopback-only Supabase HTTP double for browser verification.
// Never reads credentials, opens a database, or forwards requests to a provider.
import http from 'node:http';
import {randomUUID} from 'node:crypto';
const users=new Map(), tables=new Map();
const rows=name=>{if(!tables.has(name))tables.set(name,[]);return tables.get(name)};
const encode=x=>Buffer.from(JSON.stringify(x)).toString('base64url');
const session=user=>({access_token:`${encode({alg:'HS256',typ:'JWT'})}.${encode({sub:user.id,exp:Math.floor(Date.now()/1000)+3600,role:'authenticated',test_user:user})}.offline`,refresh_token:user.id,token_type:'bearer',expires_in:3600,user});
const authenticated=req=>{try{const payload=JSON.parse(Buffer.from((req.headers.authorization??'').split(' ')[1].split('.')[1],'base64url'));return users.get(payload.sub) ?? payload.test_user ?? null}catch{return null}};
const clean=user=>{const {password,...safe}=user;return safe};
const server=http.createServer(async(req,res)=>{
 res.setHeader('Access-Control-Allow-Origin',req.headers.origin??'*');
 res.setHeader('Access-Control-Allow-Headers','authorization,apikey,content-type,x-client-info,prefer,x-supabase-api-version');
 res.setHeader('Access-Control-Allow-Methods','GET,HEAD,POST,PUT,PATCH,DELETE,OPTIONS');
 res.setHeader('Access-Control-Expose-Headers','content-range');
 if(req.method==='OPTIONS'){res.writeHead(204);res.end();return}
 const url=new URL(req.url,'http://127.0.0.1');let raw='';for await(const chunk of req)raw+=chunk;
 let body={};try{body=raw?JSON.parse(raw):{}}catch{}
 const send=(value,status=200)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value))};
 const user=authenticated(req);
 const create=()=>{const u={id:body.id??randomUUID(),aud:'authenticated',role:'authenticated',email:body.email,password:body.password,email_confirmed_at:new Date().toISOString(),app_metadata:body.app_metadata??{},user_metadata:body.user_metadata??{},created_at:new Date().toISOString()};users.set(u.id,u);return u};
 try {
 if(url.pathname==='/auth/v1/admin/users' && req.method==='POST')return send({user:clean(create())});
 if(url.pathname==='/auth/v1/admin/users' && req.method==='GET')return send({users:[...users.values()].map(clean),aud:'authenticated'});
 if(url.pathname.startsWith('/auth/v1/admin/users/')){
  const id=url.pathname.split('/').at(-1),u=users.get(id);
  if(req.method==='DELETE'){users.delete(id);return send({user:u?clean(u):null})}
  if(req.method==='PUT'&&u)Object.assign(u,body);
  return send({user:u?clean(u):null});
 }
 if(url.pathname==='/auth/v1/token'){
  const found=url.searchParams.get('grant_type')==='refresh_token'?users.get(body.refresh_token):[...users.values()].find(u=>u.email===body.email&&u.password===body.password);
  return found?send(session(clean(found))):send({error:'invalid_grant',error_description:'Invalid login credentials'},400);
 }
 if(url.pathname==='/auth/v1/user')return user?send(clean(user)):send({message:'Invalid token'},401);
 if(url.pathname==='/auth/v1/logout')return send({});
 if(url.pathname==='/auth/v1/signup')return send({user:clean(create()),session:null});
 if(url.pathname==='/auth/v1/recover')return send({});
 if(url.pathname.startsWith('/rest/v1/rpc/')){
  if(!user)return send({code:'P0001',message:'Sign in first'},400);
  const rpc=url.pathname.split('/').at(-1);
  if(!['create_kin_family','join_kin_family'].includes(rpc))return send([]);
  if(rows('family_members').some(m=>m.user_id===user.id))return send({code:'P0001',message:'Already a family member'},400);
  let family,role='contributor';
  if(rpc==='create_kin_family'){
   family=randomUUID();rows('families').push({id:family,owner_id:user.id});rows('wearer').push({family_id:family,name:body.loved_one});
   rows('graph_nodes').push({id:randomUUID(),family_id:family,type:'person',label:body.loved_one,relation_to_wearer:'self',aliases:[]});
  }else{
   const invite=rows('family_invites').find(i=>i.token===body.invite_code && !i.used_at && new Date(i.expires_at)>new Date());
   if(!invite)return send({code:'P0001',message:'This invitation has expired or has already been used'},400);
   family=invite.family_id;role=invite.role;
   if(role==='loved_one'){
    if(rows('family_members').some(m=>m.family_id===family&&m.role==='loved_one'))return send({code:'P0001',message:'Your loved one already has an account in this family'},400);
    invite.used_at=new Date().toISOString();rows('wearer_accounts').push({user_id:user.id,family_id:family});
   }
  }
  let relative=null;
  if(role!=='loved_one'){
   if(!body.member_name||!body.relationship)return send({code:'P0001',message:'Please complete your name and relationship'},400);
   relative=randomUUID();rows('relatives').push({id:relative,family_id:family,name:body.member_name,relation_to_wearer:body.relationship,color:'#6958C9'});
  }
  rows('family_members').push({user_id:user.id,family_id:family,relative_id:relative,role});
  const stored=users.get(user.id);if(stored)stored.app_metadata={kin_family_id:family,kin_contributor_id:relative,kin_role:role==='loved_one'?'wearer':'contributor',kin_admin:rpc==='create_kin_family'};
  return send(family);
 }
 if(url.pathname.startsWith('/rest/v1/')){
  const name=url.pathname.split('/').at(-1),table=rows(name);
  const matches=row=>[...url.searchParams].every(([key,value])=>{
   if(['select','order','limit','offset'].includes(key))return true;
   if(value.startsWith('eq.'))return String(row[key])===value.slice(3);
   if(value.startsWith('gt.'))return String(row[key])>value.slice(3);
   if(value.startsWith('in.('))return value.slice(4,-1).split(',').includes(String(row[key]));
   if(value==='is.null')return row[key]==null;
   return true;
  });
  let selected=table.filter(matches);
  if(req.method==='POST'){
   selected=(Array.isArray(body)?body:[body]).map(item=>({id:randomUUID(),created_at:new Date().toISOString(),...(name==='family_invites'?{token:randomUUID(),expires_at:new Date(Date.now()+86400000).toISOString(),role:'contributor'}:{}),...item}));table.push(...selected);
  }
  if(req.method==='PATCH')selected.forEach(row=>Object.assign(row,body));
  if(req.method==='DELETE')tables.set(name,table.filter(row=>!selected.includes(row)));
  if(url.searchParams.has('order')){const [key,direction]=url.searchParams.get('order').split('.');selected.sort((a,b)=>String(a[key]).localeCompare(String(b[key]))*(direction==='desc'?-1:1))}
  if(url.searchParams.has('offset'))selected=selected.slice(Number(url.searchParams.get('offset')));
  if(url.searchParams.has('limit'))selected=selected.slice(0,Number(url.searchParams.get('limit')));
  res.setHeader('Content-Range',`0-${Math.max(0,selected.length-1)}/${selected.length}`);
  if(req.headers.accept?.includes('vnd.pgrst.object+json'))return selected.length===1?send(selected[0]):send({code:'PGRST116',details:'The result contains 0 rows'},406);
  return send(selected);
 }
 if(url.pathname.startsWith('/storage/v1/'))return send([]);
 return send({error:'Unknown offline endpoint'},404);
 }catch(error){return send({error:String(error)},500)}
});
server.listen(54329,'127.0.0.1',()=>console.log('Offline browser auth listening on 54329'));
