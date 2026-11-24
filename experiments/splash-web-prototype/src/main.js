import { createScene, handoffProgress } from './scene.js';

const duration = 4.82;
const acts = [
  { number: 'I', title: '数据之海', subtitle: 'Sea of knowledge', start: 0, cue: .21 },
  { number: 'II', title: '数据汇聚', subtitle: 'Confluence', start: .65, cue: 1.18 },
  { number: 'III', title: '人物轮廓', subtitle: 'A portrait in words', start: 1.36, cue: 2.39 },
  { number: 'IV', title: '助手显现', subtitle: 'Emergence', start: 2.54, cue: 3.23 },
];

const $ = (selector) => document.querySelector(selector);
const player = $('.player');
const canvas = $('canvas');
const stage = $('.stage');
const background = $('.shared-background');
const caption = $('.caption');
const rail = $('.rail');
const slider = $('input[type=range]');
const play = $('.play');
const status = $('.load-status');
const fullscreen = $('.fullscreen');
const exitFullscreen = $('.exit-fullscreen');
const replayButton = $('.replay');
const finalFrame = $('.final-frame');
const workspaceMode = $('.workspace-mode');
const workspace = $('#workspace-preview');
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
const format = (value) => `${value.toFixed(2)}s`;
const actButtons = [];

let scene;
let time = 0;
let playing = false;
let last = null;
let frame = 0;
let active = -1;
let captionTimer = 0;

slider.max = duration;
$('#duration').textContent = format(duration);

for (const [index, act] of acts.entries()) {
  const button = document.createElement('button');
  button.type = 'button';
  button.style.flexBasis = `${((acts[index + 1]?.start ?? duration) - act.start) / duration * 100}%`;
  const number = document.createElement('span');
  number.textContent = act.number;
  number.setAttribute('aria-hidden', 'true');
  button.append(number, act.title);
  button.addEventListener('click', () => {
    setPlaying(false);
    seek(act.cue);
  });
  $('.acts').append(button);
  actButtons.push(button);
  if (index > 0) {
    const tick = document.createElement('div');
    tick.className = 'tick';
    tick.style.left = `${act.start / duration * 100}%`;
    rail.append(tick);
  }
}

function updateCaption(index, animate) {
  if (active === index && animate) return;
  active = index;
  const act = acts[index];
  actButtons.forEach((button, i) => {
    if (i === index) button.setAttribute('aria-current', 'step');
    else button.removeAttribute('aria-current');
  });
  function writeCaption() {
    $('#act-number').textContent = act.number;
    $('#act-title').textContent = act.title;
    $('#act-subtitle').textContent = act.subtitle;
    caption.classList.remove('changing');
  }
  clearTimeout(captionTimer);
  if (animate && !reducedMotion.matches) {
    caption.classList.add('changing');
    captionTimer = setTimeout(writeCaption, 94);
  } else {
    writeCaption();
  }
}

function render(animateCaption = false) {
  scene?.draw(time);
  const dawn = handoffProgress(time, duration);
  stage.style.setProperty('--intro-chrome', Math.max(0, 1 - dawn * 2.5));
  stage.classList.toggle('shared-frame', time === duration);
  finalFrame.setAttribute('aria-pressed', String(time === duration && workspace.hidden));
  workspaceMode.setAttribute('aria-pressed', String(!workspace.hidden));
  const index = acts.findLastIndex((act) => time >= act.start);
  slider.value = time;
  rail.style.setProperty('--progress', time / duration);
  $('#elapsed').textContent = format(time);
  updateCaption(index, animateCaption);
}

function setPlaying(next) {
  if (!scene || playing === next) return;
  playing = next;
  play.classList.toggle('playing', next);
  play.setAttribute('aria-label', next ? '暂停' : '播放');
  play.title = next ? '暂停 (空格)' : '播放 (空格)';
  last = null;
  if (playing) frame = requestAnimationFrame(tick);
  else cancelAnimationFrame(frame);
}

function tick(now) {
  if (last !== null) time = Math.min(duration, time + (now - last) / 1000);
  last = now;
  render(true);
  if (time >= duration) setPlaying(false);
  else frame = requestAnimationFrame(tick);
}

function seek(next, showWorkspace = false) {
  time = Math.max(0, Math.min(duration, next));
  workspace.hidden = !showWorkspace;
  last = null;
  render();
  if (time === duration && playing) setPlaying(false);
}

function toggle() {
  if (time >= duration) seek(0);
  setPlaying(!playing);
}

function replay() {
  seek(0);
  setPlaying(true);
}

async function toggleFullscreen() {
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await player.requestFullscreen();
  } catch (error) {
    console.warn('Fullscreen unavailable', error);
  }
}

slider.addEventListener('input', () => {
  setPlaying(false);
  seek(Number(slider.value));
});
play.addEventListener('click', toggle);
replayButton.addEventListener('click', replay);
finalFrame.addEventListener('click', () => {
  setPlaying(false);
  seek(duration);
});
workspaceMode.addEventListener('click', () => {
  setPlaying(false);
  seek(duration, workspace.hidden);
});
fullscreen.addEventListener('click', toggleFullscreen);
exitFullscreen.addEventListener('click', toggleFullscreen);
fullscreen.hidden = !document.fullscreenEnabled;
document.addEventListener('visibilitychange', () => { last = null; });

document.addEventListener('keydown', (event) => {
  if (event.ctrlKey || event.altKey || event.metaKey || event.repeat) return;
  if (event.code === 'Space' && !event.target.closest('input,button')) {
    event.preventDefault();
    toggle();
  }
  if (event.code === 'KeyR') replay();
  if (event.code === 'KeyF' && document.fullscreenEnabled) toggleFullscreen();
});

try {
  await background.decode();
  scene = createScene(canvas, { duration, background });
  new ResizeObserver(() => {
    scene.resize();
    render();
  }).observe(stage);
  status.hidden = true;
  stage.classList.add('scene-ready');
  finalFrame.disabled = workspaceMode.disabled = false;
  render();
  setPlaying(!reducedMotion.matches);
} catch (error) {
  console.error(error);
  status.textContent = '演出加载失败。请在支持 WebGL 2 的浏览器中通过本地预览地址打开。';
  play.disabled = replayButton.disabled = slider.disabled = true;
  actButtons.forEach((button) => { button.disabled = true; });
}
