import {test} from 'node:test';
import assert from 'node:assert/strict';
import {webcrypto} from 'node:crypto';
import {SessionClient,SessionError,openSessionSocket,closeSessionSocket,sendSessionMessage,noteSessionPong} from '../../src/browser_session.mjs';
const base='http://127.0.0.1:8000/api';
const identity={user_id:'11111111-1111-4111-8111-111111111111',nickname:'玩家',created_at:1};
const current={jwt:'test-access',identity,session_id:'22222222-2222-4222-8222-222222222222',refresh_token:'a'.repeat(64)};
function storage(){const data=new Map();return {getItem:k=>data.get(k)??null,setItem:(k,v)=>data.set(k,v)};}
function locks(){let tail=Promise.resolve();return {request:(_key,work)=>{const job=tail.then(work);tail=job.catch(()=>{});return job;}};}
const response=(data,status=200)=>({status,ok:status>=200&&status<300,json:async()=>structuredClone(data)});
function client(fetcher,store=storage(),lock=locks()) {return new SessionClient(base,{fetch:fetcher,storage:store,locks:lock,crypto:webcrypto});}
function cached(c){c.save({kind:'session',value:current});}

test('network and non-401 errors preserve credentials and never create identities',async()=>{
    for(const status of [0,400,403,429,500,503]){
        const seen=[];const c=client(async url=>{seen.push(url);if(!status)throw Error('offline');return response({},status);});cached(c);
        const before=c.storage.getItem(c.key);
        await assert.rejects(c.initialize(),SessionError);
        assert(c.storage.getItem(c.key)===before);assert(seen.length===1&&seen[0].endsWith('/verify'));
    }
});
test('401 rotates within the same identity and saves the new credentials',async()=>{
    let calls=0;const c=client(async(url,opts)=>{
        calls++;if(url.endsWith('/verify'))return response({},401);
        const body=JSON.parse(opts.body);assert(url.endsWith('/refresh'));
        return response({...current,jwt:'renewed',refresh_token:body.next_refresh_token});
    });cached(c);const value=await c.initialize();assert.equal(value.identity.user_id,identity.user_id);assert.equal(calls,2);assert(value.refresh_token!==current.refresh_token);
});
test('lost rotation response retries the identical pending request',async()=>{
    let payload,rotations=0,attempts=0;
    const c=client(async(url,opts)=>{
        if(url.endsWith('/verify'))return response({},401);
        attempts++;const body=JSON.parse(opts.body);
        if(!payload){payload=body;rotations++;throw Error('response lost after commit');}
        assert(JSON.stringify(payload)===JSON.stringify(body));
        return response({...current,jwt:'renewed',refresh_token:body.next_refresh_token});
    });cached(c);await assert.rejects(c.initialize());assert.equal(c.load().kind,'rotate');
    const value=await c.initialize();assert.equal(value.identity.user_id,identity.user_id);assert.equal(rotations,1);assert.equal(attempts,2);
});
test('lost create response retries the original candidate after page reload',async()=>{
    let candidate,created=0;
    const fetcher=async(url,opts)=>{
        assert(url.endsWith('/create'));const body=JSON.parse(opts.body);
        if(!candidate){candidate=body;created++;throw Error('lost');}
        assert(JSON.stringify(candidate)===JSON.stringify(body));return response({...current,...body});
    };
    const store=storage();await assert.rejects(client(fetcher,store).initialize());
    await client(fetcher,store).initialize();assert.equal(created,1);
});
test('two tabs share a lock and only one performs a refresh',async()=>{
    const store=storage(),lock=locks();let refreshes=0;
    const fetcher=async(url,opts)=>{
        if(url.endsWith('/verify'))return opts.headers.Authorization==='Bearer renewed'?response({identity}):response({},401);
        refreshes++;const body=JSON.parse(opts.body);return response({...current,jwt:'renewed',refresh_token:body.next_refresh_token});
    };
    const a=client(fetcher,store,lock),b=client(fetcher,store,lock);cached(a);
    const result=await Promise.all([a.initialize(),b.initialize()]);assert.equal(refreshes,1);assert(result.every(v=>v.jwt==='renewed'));
});
test('revoked refresh stays recoverable locally and does not create another account',async()=>{
    const urls=[];const c=client(async url=>{urls.push(url);return response({},401);});cached(c);
    await assert.rejects(c.initialize());assert.equal(c.load().kind,'rotate');assert(!urls.some(url=>url.endsWith('/create')));
});
test('storage failure and unsupported cross-tab locks stop before network side effects',async()=>{
    let calls=0;const fetcher=async()=>{calls++;return response({});};
    const store=storage();store.setItem=()=>{throw Error('quota');};
    await assert.rejects(client(fetcher,store).initialize());await assert.rejects(client(fetcher,storage(),{}).initialize());assert.equal(calls,0);
});
test('legacy tokens use controlled exchange and never anonymous fallback',async()=>{
    const store=storage();store.setItem('game_jwt_token','test-legacy');let calls=0;
    const c=client(async url=>{calls++;assert(url.endsWith('/session'));return response({},401);},store);
    await assert.rejects(c.initialize());assert.equal(c.load().kind,'legacy');assert.equal(calls,1);assert.equal(store.getItem('game_jwt_token'),'test-legacy');
});

function socketFixture(t) {
    const original = {WebSocket:globalThis.WebSocket,dispatchEvent:globalThis.dispatchEvent};
    const events=[];
    class Socket {
        static OPEN=1;
        static instances=[];
        constructor() {this.readyState=1;this.bufferedAmount=0;this.sent=[];Socket.instances.push(this);}
        send(bytes) {this.sent.push(new Uint8Array(bytes));}
        close(code=1000) {if(this.readyState===3)return;this.readyState=3;this.code=code;this.onclose?.({code});}
    }
    globalThis.WebSocket=Socket;globalThis.dispatchEvent=e=>{events.push(e);return true;};
    t.mock.timers.enable({apis:['setTimeout','setInterval','Date']});
    t.after(()=>{closeSessionSocket();globalThis.WebSocket=original.WebSocket;globalThis.dispatchEvent=original.dispatchEvent;});
    return {Socket,events,async open() {
        const promise=openSessionSocket('ws://test.invalid',new Uint8Array([1]),new Uint8Array([2]));
        const ws=Socket.instances.at(-1);ws.onopen();
        ws.onmessage({data:new Uint8Array([8,1]).buffer});await promise;return ws;
    }};
}
test('continuous pushes cannot replace Pong and the watchdog ends a silent connection',async t=>{
    const f=socketFixture(t);const ws=await f.open();
    for(let i=0;i<44;i++) {
        ws.onmessage({data:new Uint8Array([8,1,98,0]).buffer});
        t.mock.timers.tick(1000);
    }
    assert.equal(ws.readyState,1);
    t.mock.timers.tick(1000);
    assert.equal(ws.code,4000);
    assert.equal(f.events.at(-1).type,'patchwork-session-ended');
    assert.equal(f.events.at(-1).detail,true);
});
test('Pong extends liveness, client send buffers are bounded and takeover is not retried',async t=>{
    const f=socketFixture(t);let ws=await f.open();
    t.mock.timers.tick(30000);noteSessionPong();t.mock.timers.tick(30000);
    assert.equal(ws.readyState,1);
    ws.bufferedAmount=65537;
    assert.throws(()=>sendSessionMessage(new Uint8Array([1])));
    assert.equal(ws.code,4000);
    closeSessionSocket();ws=await f.open();ws.close(1008);
    assert.equal(f.events.at(-1).detail,false);
    closeSessionSocket();ws=await f.open();
    assert.throws(()=>sendSessionMessage(new Uint8Array(16385)));
    assert.equal(ws.code,4000);
});
