// Local preview-only observation helper. No session data or network payloads.
// Copy beside a preview index and load from a separate probe HTML, never production.
const selectors = ['.room-entry', '.room-heading', '.room-seats', '.room-actions', '#game-canvas'];
// Optional bounded delay exercises pending feedback without changing the backend.
const replyDelayMs = ['localhost', '127.0.0.1'].includes(location.hostname)
    ? Math.min(2000, Math.max(0, Number(new URL(location.href).searchParams.get('reply-delay')) || 0)) : 0;
if (replyDelayMs) {
    const NativeWebSocket = globalThis.WebSocket;
    globalThis.WebSocket = class extends NativeWebSocket {
        set onmessage(handler) {
            super.onmessage = handler ? event => setTimeout(() => handler.call(this, event), replyDelayMs) : null;
        }
        get onmessage() { return super.onmessage; }
    };
}
const nodeIds = new WeakMap();
let nextId = 1;
function snapshot() {
    return Object.fromEntries(selectors.map(selector => {
        const element = document.querySelector(selector);
        if (!element) return [selector, null];
        if (!nodeIds.has(element)) nodeIds.set(element, nextId++);
        const rect = element.getBoundingClientRect();
        return [selector, {node: nodeIds.get(element), x: rect.x, y: rect.y,
            width: rect.width, height: rect.height}];
    }));
}
const reports = [];
let active = false;
document.addEventListener('click', event => {
    const button = event.target.closest('.friend-rooms button');
    const app = document.querySelector('.app-container');
    if (!button || !app || active) return;
    active = true;
    const initial = snapshot();
    const report = {action: button.textContent.trim(), samples: 0, maxPositionDelta: {},
        replaced: [], opacityMin: 1, canvasSizeWrites: 0, pendingVisibleSamples: 0, replyDelayMs};
    const sample = () => {
        report.samples++;
        if (button.isConnected) report.opacityMin = Math.min(report.opacityMin, Number(getComputedStyle(button).opacity));
        const pending = document.querySelector('.room-pending');
        if (pending && getComputedStyle(pending).visibility === 'visible') report.pendingVisibleSamples++;
        const current = snapshot();
        for (const selector of selectors) {
            const first = initial[selector], now = current[selector];
            if (!first || !now) continue;
            report.maxPositionDelta[selector] = Math.max(report.maxPositionDelta[selector] || 0,
                Math.abs(now.x - first.x), Math.abs(now.y - first.y),
                Math.abs(now.width - first.width), Math.abs(now.height - first.height));
            if (first.node !== now.node && !report.replaced.includes(selector)) report.replaced.push(selector);
        }
    };
    const observer = new MutationObserver(records => {
        report.canvasSizeWrites += records.filter(record =>
            record.target.id === 'game-canvas' && ['width', 'height'].includes(record.attributeName)).length;
        sample();
    });
    observer.observe(app, {subtree: true, childList: true, attributes: true, characterData: true});
    sample();
    const frame = () => { if (active) { sample(); requestAnimationFrame(frame); } };
    requestAnimationFrame(frame);
    setTimeout(() => {
        sample(); active = false; observer.disconnect();
        reports.push(report);
        let output = document.querySelector('#layout-probe-output');
        if (!output) {
            output = document.createElement('pre');
            output.id = 'layout-probe-output';
            output.style.cssText = 'position:fixed;bottom:4px;right:4px;max-width:460px;max-height:160px;overflow:auto;z-index:999;background:#fff;color:#111;font:11px monospace;padding:8px';
            document.body.append(output);
        }
        output.textContent = JSON.stringify(reports, null, 2);
    }, replyDelayMs + 900);
}, true);
