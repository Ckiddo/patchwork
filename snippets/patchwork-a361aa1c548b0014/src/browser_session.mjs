// This module owns browser credential storage. Never log requests or responses.
export class SessionError extends Error {
    constructor(kind) { super(kind); this.kind = kind; }
}
export class SessionClient {
    constructor(base, {storage, fetch: fetcher, crypto, locks}) {
        this.base = base.replace(/\/$/, ''); this.storage = storage;
        this.fetcher = fetcher; this.crypto = crypto; this.locks = locks;
        this.key = `patchwork_session_v1:${this.base}`;
    }
    candidate() {
        const bytes = this.crypto.getRandomValues(new Uint8Array(32));
        return {session_id: this.crypto.randomUUID(), refresh_token: Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('')};
    }
    save(record) { try { this.storage.setItem(this.key, JSON.stringify(record)); } catch { throw new SessionError('storage'); } }
    load() {
        try {
            const raw = this.storage.getItem(this.key);
            if (!raw) return null;
            const record = JSON.parse(raw);
            if (!['session','create','legacy','rotate'].includes(record.kind)) throw Error();
            return record;
        } catch { throw new SessionError('storage'); }
    }
    async request(path, {jwt, body} = {}) {
        const controller = new AbortController(); const timeout = setTimeout(() => controller.abort(), 12000);
        try {
            const headers = {'Content-Type':'application/json'};
            if (jwt) headers.Authorization = `Bearer ${jwt}`;
            let response;
            try { response = await this.fetcher(this.base + path, {method:'POST',headers,body:body ? JSON.stringify(body) : undefined,cache:'no-store',signal:controller.signal}); }
            catch { throw new SessionError('network'); }
            if (response.status === 401) throw new SessionError('unauthorized');
            if (response.status === 403) throw new SessionError('blocked');
            if (response.status === 429) throw new SessionError('rate_limited');
            if (!response.ok) throw new SessionError('server');
            try { return await response.json(); } catch { throw new SessionError('response'); }
        } finally { clearTimeout(timeout); }
    }
    accept(value, expectedSession, expectedUser) {
        if (typeof value.jwt !== 'string' || !value.jwt || value.session_id !== expectedSession ||
            !/^[0-9a-f]{64}$/.test(value.refresh_token) || typeof value.identity?.user_id !== 'string' ||
            typeof value.identity?.nickname !== 'string' || (expectedUser && value.identity.user_id !== expectedUser)) throw new SessionError('response');
        this.save({kind:'session',value}); return value;
    }
    async pending(record) {
        try { return await this.completePending(record); }
        catch (error) {
            if (error instanceof SessionError && error.kind === 'unauthorized') {
                if (record.kind === 'legacy') throw new SessionError('legacy_unavailable');
                if (record.kind === 'rotate') throw new SessionError('session_expired');
            }
            throw error;
        }
    }
    async completePending(record) {
        if (record.kind === 'rotate') {
            const value = await this.request('/auth/refresh', {body:record.rotation});
            if (value.refresh_token !== record.rotation.next_refresh_token) throw new SessionError('response');
            return this.accept(value, record.value.session_id, record.value.identity.user_id);
        }
        const value = await this.request(record.kind === 'legacy' ? '/auth/session' : '/auth/create', {jwt:record.legacy_jwt,body:record.candidate});
        if (value.refresh_token !== record.candidate.refresh_token) throw new SessionError('response');
        return this.accept(value, record.candidate.session_id);
    }
    async initialize({replaceRejected = false} = {}) {
        if (!this.base || !/^https?:\/\//.test(this.base)) throw new SessionError('configuration');
        if (!this.locks?.request) throw new SessionError('locks');
        return this.locks.request(this.key, async () => {
            // Recheck under the same lock: another tab or a recovered service may
            // already have restored this identity while the confirmation was open.
            try { return await this.initializeLocked(); }
            catch (error) {
                if (!replaceRejected || !(error instanceof SessionError) ||
                    !['legacy_unavailable','session_expired'].includes(error.kind)) throw error;
            }
            // Only an explicit user action can replace a definitively rejected
            // identity. Persist the full old record before changing the active one.
            try {
                const previous = this.storage.getItem(this.key);
                if (!previous) throw Error();
                this.storage.setItem(`${this.key}:backup:${this.crypto.randomUUID()}`, previous);
            } catch { throw new SessionError('storage'); }
            const record = {kind:'create',candidate:this.candidate()};
            this.save(record);
            return this.pending(record);
        });
    }
    async initializeLocked() {
        let record = this.load();
        if (!record) {
            let legacy; try { legacy = this.storage.getItem('game_jwt_token'); } catch { throw new SessionError('storage'); }
            record = {kind:legacy ? 'legacy' : 'create',candidate:this.candidate(),...(legacy ? {legacy_jwt:legacy} : {})};
            this.save(record); // Save before the first side effect, including first identity creation.
        }
        if (record.kind !== 'session') return this.pending(record);
        const current = record.value;
        if (!current?.session_id || !current?.refresh_token || !current?.jwt || !current?.identity?.user_id) throw new SessionError('storage');
        try {
            const verified = await this.request('/auth/verify', {jwt:current.jwt});
            if (verified.identity?.user_id !== current.identity.user_id) throw new SessionError('response');
            const value = {...current,identity:verified.identity}; this.save({kind:'session',value}); return value;
        } catch (error) {
            if (!(error instanceof SessionError) || error.kind !== 'unauthorized') throw error;
        }
        const rotation = {session_id:current.session_id,refresh_token:current.refresh_token,next_refresh_token:this.candidate().refresh_token,rotation_id:this.crypto.randomUUID()};
        record = {kind:'rotate',value:current,rotation}; this.save(record);
        return this.pending(record);
    }
}
async function browserSession(base, options) {
    try { return await new SessionClient(base, {storage:globalThis.localStorage,fetch:globalThis.fetch.bind(globalThis),crypto:globalThis.crypto,locks:globalThis.navigator.locks}).initialize(options); }
    catch (error) { throw new Error(error instanceof SessionError ? error.kind : 'storage'); }
}
export function initializeSession(base) { return browserSession(base); }
export function startNewSession(base) { return browserSession(base, {replaceRejected:true}); }
let socket;
let pongAt = 0;
export function noteSessionPong() { pongAt = Date.now(); }
export function reconnectDelay(attempt) {
    const delay = Math.min(30000, 500 * 2 ** Math.min(attempt, 6));
    return new Promise(resolve => setTimeout(resolve, delay + Math.random() * 250));
}
export async function openSessionSocket(url, auth, ping) {
    // Keep owned copies after Rust releases its WASM byte buffers.
    auth = new Uint8Array(auth); ping = new Uint8Array(ping);
    if (socket) socket.close();
    return new Promise((resolve, reject) => {
        const current = new WebSocket(url); socket = current; current.binaryType = 'arraybuffer';
        let authenticated = false;
        const timeout = setTimeout(() => { current.close(); reject(new Error('socket')); }, 8000);
        let pingAt = Date.now();
        const heartbeat = setInterval(() => {
            if (!authenticated || current.readyState !== WebSocket.OPEN) return;
            if (Date.now() - pongAt >= 45000 || current.bufferedAmount > 65536) { current.close(4000, 'heartbeat timeout'); return; }
            if (Date.now() - pingAt >= 15000) { current.send(ping); pingAt = Date.now(); }
        }, 1000);
        current.onopen = () => current.send(auth);
        current.onerror = () => { clearTimeout(timeout); current.close(); reject(new Error('socket')); };
        current.onmessage = event => {
            if (!authenticated && event.data instanceof ArrayBuffer) {
                clearTimeout(timeout); authenticated = true; pongAt = Date.now(); resolve(new Uint8Array(event.data));
            } else if (authenticated && event.data instanceof ArrayBuffer) {
                if (event.data.byteLength > 65536) { current.close(1008, 'message limit'); return; }
                globalThis.dispatchEvent(new CustomEvent('patchwork-message', {detail:new Uint8Array(event.data)}));
            }
        };
        current.onclose = event => {
            clearTimeout(timeout); clearInterval(heartbeat);
            if (!authenticated) reject(new Error('socket'));
            else if (socket === current) globalThis.dispatchEvent(new CustomEvent('patchwork-session-ended', {detail: event.code !== 1008}));
        };
    });
}
export function closeSessionSocket() { if (socket) { const previous=socket; socket=null; previous.close(); } }
export function sendSessionMessage(bytes) {
    if (!socket || socket.readyState !== WebSocket.OPEN) throw new Error('socket');
    if (bytes.length > 16384 || socket.bufferedAmount > 65536) {
        socket.close(4000, 'send queue limit'); throw new Error('backpressure');
    }
    socket.send(new Uint8Array(bytes));
}
export function newRequestId() { return globalThis.crypto.randomUUID(); }
export function saveRoomPending(key, value) { globalThis.sessionStorage.setItem(key, value); }
export function loadRoomPending(key) { return globalThis.sessionStorage.getItem(key); }
export function clearRoomPending(key) { globalThis.sessionStorage.removeItem(key); }
