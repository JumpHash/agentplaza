// Unit tests for the parity harness's PNG codec and perceptual diff. Run with:
//   node --test scripts/parity/diff.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { diff } from './diff.mjs';
import { decode, encode } from './png.mjs';

/** A width x height RGBA image filled by fn(x, y) -> [r, g, b, a]. */
function image(width, height, fn) {
  const data = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      data.set(fn(x, y), (y * width + x) * 4);
    }
  }
  return { width, height, data };
}

const WHITE = [255, 255, 255, 255];
const RED = [255, 0, 0, 255];

test('identical images have no mismatched pixels', () => {
  const a = image(100, 100, (x, y) => [x * 2, y * 2, (x + y) & 255, 255]);
  const b = image(100, 100, (x, y) => [x * 2, y * 2, (x + y) & 255, 255]);
  const result = diff(a, b);
  assert.equal(result.mismatched, 0);
  assert.equal(result.total, 10000);
  assert.equal(result.ratio, 0);
});

test('a 10 by 10 red block on a 100 by 100 white image is a ratio of about 0.01', () => {
  const a = image(100, 100, () => WHITE);
  const b = image(100, 100, (x, y) => (x >= 20 && x < 30 && y >= 20 && y < 30 ? RED : WHITE));
  const result = diff(a, b);
  assert.equal(result.mismatched, 100);
  assert.ok(Math.abs(result.ratio - 0.01) < 0.0005, `ratio ${result.ratio}`);
  assert.deepEqual(result.image.data.subarray(0, 4), new Uint8Array([255, 255, 255, 255]));
});

test('a difference inside the threshold is not counted', () => {
  const a = image(50, 50, () => [100, 100, 100, 255]);
  const b = image(50, 50, () => [101, 100, 100, 255]);
  assert.equal(diff(a, b, { threshold: 0.1 }).mismatched, 0);
});

test('images of different sizes compare at their smaller common size', () => {
  const a = image(80, 60, () => WHITE);
  const b = image(100, 100, () => WHITE);
  const result = diff(a, b);
  assert.equal(result.total, 80 * 60);
  assert.equal(result.mismatched, 0);
  assert.equal(result.image.width, 80);
  assert.equal(result.image.height, 60);
});

test('encode then decode gives back the same pixels', () => {
  const src = image(17, 9, (x, y) => [(x * 13) & 255, (y * 29) & 255, (x * y) & 255, 255 - x]);
  const back = decode(encode(src));
  assert.equal(back.width, 17);
  assert.equal(back.height, 9);
  assert.deepEqual(Buffer.from(back.data), Buffer.from(src.data));
});

test('decode rejects data that is not a PNG', () => {
  assert.throws(() => decode(Buffer.from('not a png file at all')), /not a PNG/);
});
