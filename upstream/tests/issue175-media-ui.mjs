// Execute the shipped QML JavaScript methods without fetching Qt test libraries.
// Native QtTest and visual acceptance remain separate gates.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
const picker = process.argv[2];
if (!picker) throw new Error('Pass the patched picker source directory');
function functions(file) {
    const source = fs.readFileSync(file, 'utf8');
    const matches = [...source.matchAll(/^    function \w+\([^\n]*\) \{\n[\s\S]*?^    \}/gm)];
    assert.ok(matches.length > 0, `No methods in ${file}`);
    return matches.map(match => match[0]).join('\n');
}
const trash = vm.createContext({receipts: [], busy: false});
Object.defineProperties(trash, {
    canDelete: {get: () => !trash.busy},
    canRestore: {get: () => !trash.busy && trash.receipts.length > 0},
});
const spy = () => ({count: 0, clear() { this.count = 0; }});
const errors = spy(), deletions = spy(), restores = spy();
const rpc = {
    pending: null, method: '', params: null, calls: 0,
    call(method, params, callback) {
        this.method = method; this.params = params; this.pending = callback; ++this.calls;
    },
};
Object.assign(trash, {
    trash, transport: rpc,
    failed() { ++errors.count; }, deleted() { ++deletions.count; }, restored() { ++restores.count; },
});
vm.runInContext(functions(path.join(picker, 'qml/MediaTrashState.qml')), trash);
const tests = vm.createContext({
    trash, rpc, errors, deletions, restores,
    receipt: 'a'.repeat(64) + '.' + 'b'.repeat(64),
    compare: assert.equal, verify: assert.ok,
});
vm.runInContext(functions(path.join(picker, 'tests/tst_MediaTrashState.qml')), tests);
let count = 0;
for (const name of Object.keys(tests).filter(name => name.startsWith('test_'))) {
    tests.init(); tests[name](); ++count; console.log(`PASS ${name}`);
}
assert.equal(count, 6);
// These shipped controls call the tested state object, including retry visibility.
const card = fs.readFileSync(path.join(picker, 'qml/chrome/CardBackPanel.qml'), 'utf8');
const toast = fs.readFileSync(path.join(picker, 'qml/chrome/ToastLayer.qml'), 'utf8');
assert.ok(card.includes('back.state.mediaTrash.deleteKey(back.cardKey)'));
assert.ok(card.includes('enabled: back.state.mediaTrash.canDelete'));
assert.ok(toast.includes('onTriggered: layer.state.mediaTrash.restoreLast()'));
assert.ok(toast.includes('enabled: layer.state.mediaTrash.canRestore'));
const state = fs.readFileSync(path.join(picker, 'qml/PickerState.qml'), 'utf8');
const host = fs.readFileSync(path.join(picker, 'qml/SheetHost.qml'), 'utf8');
const sheet = fs.readFileSync(path.join(picker, 'qml/sheets/MediaTrashSheet.qml'), 'utf8');
assert.ok(state.includes('state.mediaTrash.refresh()'));
assert.ok(toast.includes('layer.state.openSheet("mediaTrash")'));
assert.ok(host.includes('sheetName: "mediaTrash"'));
assert.ok(sheet.includes('root.state.mediaTrash.restoreReceipt(modelData.receipt)'));
assert.ok(sheet.includes('root.state.mediaTrash.previousPage()'));
assert.ok(sheet.includes('root.state.mediaTrash.nextPage()'));
console.log('PASS shipped delete, undo, refresh, paginated trash and selected restore bindings');
