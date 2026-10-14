import init, { shared_layout } from './apz_engine_web-a39519b8.js';
const ready = init();
onmessage = async (e) => {
  await ready;
  let answer;
  try { answer = shared_layout(e.data); } catch (err) { answer = JSON.stringify({ err: String(err) }); }
  postMessage({ key: e.data, answer });
};
