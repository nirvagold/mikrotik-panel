import { test } from 'node:test';
import assert from 'node:assert/strict';
import { keyIntent, parseSlash } from './composer.ts';

const idle = { empty: false, running: false };
test('Enter mengirim, Shift+Enter baris baru', () => {
  assert.equal(keyIntent({ key: 'Enter', shiftKey: false }, idle), 'send');
  assert.equal(keyIntent({ key: 'Enter', shiftKey: true }, idle), 'newline');
  assert.equal(keyIntent({ key: 'Enter', shiftKey: false }, { empty: true, running: false }), 'none');
});
test('IME tidak mengirim', () => {
  assert.equal(keyIntent({ key: 'Enter', shiftKey: false, isComposing: true }, idle), 'none');
  assert.equal(keyIntent({ key: 'Enter', shiftKey: false, keyCode: 229 }, idle), 'none');
});
test('Esc menghentikan, panah atas mengedit pesan terakhir', () => {
  assert.equal(keyIntent({ key: 'Escape', shiftKey: false }, { empty: false, running: true }), 'stop');
  assert.equal(keyIntent({ key: 'ArrowUp', shiftKey: false }, { empty: true, running: false }), 'edit-last');
  assert.equal(keyIntent({ key: 'ArrowUp', shiftKey: false }, idle), 'none');
});
test('perintah garis miring', () => {
  assert.deepEqual(parseSlash('/undo'), { command: 'undo' });
  assert.deepEqual(parseSlash('/tema Gelap'), { command: 'theme', arg: 'dark' });
  assert.equal(parseSlash('/tema ungu'), null);
  assert.equal(parseSlash('ubah warna'), null);
});
