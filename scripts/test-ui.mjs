// Isolated browser tests: no production Supabase or paid provider credentials.
import { spawn } from 'node:child_process';
import net from 'node:net';
const ports = [54329, 3102, 8788];
for (const port of ports) await new Promise((resolve, reject) => {
  const server = net.createServer();
  server.once('error', () => reject(new Error(`Port ${port} is busy. Stop its local process before running test:e2e.`)));
  server.listen(port, '127.0.0.1', () => server.close(resolve));
});
const env = {...process.env, NEXT_PUBLIC_SUPABASE_URL:'http://127.0.0.1:54329',
  NEXT_PUBLIC_SUPABASE_ANON_KEY:'offline-anon', NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY:'offline-anon',
  SUPABASE_SERVICE_ROLE_KEY:'offline-service', SUPABASE_SECRET_KEY:'offline-service',
  OPENAI_API_KEY:'', DEEPGRAM_API_KEY:'', ELEVENLABS_API_KEY:'', KIN_FACE_SERVICE_URL:'', KIN_FACE_SERVICE_TOKEN:'',
  KIN_BRAIN_PORT:'8788', KIN_BACKEND_URL:'http://127.0.0.1:8788', KIN_FACE_MODEL:'', KIN_FACE_TOKEN_KEY:'ab'.repeat(32), PLAYWRIGHT_BASE_URL:'http://localhost:3102'};
const children=[];
const start=(command,args)=>{const child=spawn(command,args,{env,stdio:'inherit'});children.push(child);return child};
const stop=()=>{for(const child of children) child.kill('SIGTERM')};
process.on('SIGINT',()=>{stop();process.exit(130)});
process.on('SIGTERM',()=>{stop();process.exit(143)});
try {
  start(process.execPath,['scripts/ui-test-auth.mjs']);
  start('cargo',['run','--locked','--manifest-path','brain/Cargo.toml']);
  start(process.execPath,['node_modules/next/dist/bin/next',process.env.KIN_UI_PRODUCTION==='1'?'start':'dev','-p','3102']);
  const deadline=Date.now()+600000;
  while(true){
    if(children.some(child=>child.exitCode!==null))throw new Error('A test server stopped early');
    try{
      const [web,backend]=await Promise.all([
        fetch(env.PLAYWRIGHT_BASE_URL,{signal:AbortSignal.timeout(3000)}),
        fetch(`${env.KIN_BACKEND_URL}/health`,{signal:AbortSignal.timeout(3000)}),
      ]);
      if(web.ok&&backend.ok)break;
    }catch{}
    if(Date.now()>deadline)throw new Error('Local test server did not start');
    await new Promise(resolve=>setTimeout(resolve,500));
  }
  const run=start(process.execPath,['node_modules/@playwright/test/cli.js','test',...process.argv.slice(2)]);
  process.exitCode=await new Promise(resolve=>run.once('exit',code=>resolve(code??1)));
}finally{stop()}
