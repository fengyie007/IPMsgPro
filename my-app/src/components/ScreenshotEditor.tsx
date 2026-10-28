// Adapted from the C++ frontend editor: annotations are stored in image pixels.
import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { FiSquare, FiArrowUpRight, FiEdit2, FiGrid, FiType, FiRotateCcw, FiMaximize, FiX, FiCheck } from 'react-icons/fi';
import { imagePoint, imageSelection, type Point, type Selection } from '../utils/screenshot';

type Tool = 'rect' | 'arrow' | 'pencil' | 'mosaic' | 'text';
type Annotation =
  | ({ type: 'rect' | 'mosaic'; color: string; width: number } & Selection)
  | { type: 'arrow'; start: Point; end: Point; color: string; width: number }
  | { type: 'pencil'; points: Point[]; color: string; width: number }
  | { type: 'text'; point: Point; text: string; color: string; width: number };
const colors = ['#ff3b30', '#ff9500', '#ffcc00', '#34c759', '#007aff', '#ffffff', '#000000'];
const tools = [
  { value: 'rect', label: '矩形', Icon: FiSquare }, { value: 'arrow', label: '箭头', Icon: FiArrowUpRight },
  { value: 'pencil', label: '画笔', Icon: FiEdit2 }, { value: 'mosaic', label: '马赛克', Icon: FiGrid },
  { value: 'text', label: '文字', Icon: FiType },
] as const;

function drawAnnotation(ctx: CanvasRenderingContext2D, a: Annotation, base: HTMLImageElement, selection: Selection) {
  ctx.save();
  ctx.strokeStyle = a.color; ctx.fillStyle = a.color; ctx.lineWidth = a.width;
  ctx.lineCap = 'round'; ctx.lineJoin = 'round';
  if (a.type === 'rect') ctx.strokeRect(a.x, a.y, a.w, a.h);
  else if (a.type === 'arrow') {
    const angle = Math.atan2(a.end.y - a.start.y, a.end.x - a.start.x), head = Math.max(12, a.width * 4);
    ctx.beginPath(); ctx.moveTo(a.start.x, a.start.y); ctx.lineTo(a.end.x, a.end.y);
    for (const offset of [-0.85, 0.85]) {
      ctx.moveTo(a.end.x, a.end.y);
      ctx.lineTo(a.end.x + head * Math.cos(angle + offset * Math.PI), a.end.y + head * Math.sin(angle + offset * Math.PI));
    }
    ctx.stroke();
  } else if (a.type === 'pencil') {
    ctx.beginPath();
    a.points.forEach((p, i) => { if (i === 0) ctx.moveTo(p.x, p.y); else ctx.lineTo(p.x, p.y); });
    ctx.stroke();
  } else if (a.type === 'mosaic' && a.w > 0 && a.h > 0) {
    const tile = document.createElement('canvas');
    tile.width = Math.max(1, Math.floor(a.w / Math.max(8, a.width * 4)));
    tile.height = Math.max(1, Math.floor(a.h / Math.max(8, a.width * 4)));
    const tiny = tile.getContext('2d')!;
    tiny.drawImage(base, selection.x + a.x, selection.y + a.y, a.w, a.h, 0, 0, tile.width, tile.height);
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(tile, 0, 0, tile.width, tile.height, a.x, a.y, a.w, a.h);
  } else if (a.type === 'text') {
    ctx.font = `${a.width}px "Microsoft YaHei", sans-serif`; ctx.textBaseline = 'top';
    a.text.split('\n').forEach((line, i) => ctx.fillText(line, a.point.x, a.point.y + i * a.width * 1.3));
  }
  ctx.restore();
}

export default function ScreenshotEditor({ image, busy, onReady, onCancel, onConfirm, onFailure }: {
  image: string; busy: boolean; onReady: () => void; onCancel: () => void;
  onConfirm: (blob: Blob) => Promise<void>; onFailure: (error: string) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const areaRef = useRef<HTMLDivElement>(null);
  const baseRef = useRef<HTMLImageElement | null>(null);
  const dragRef = useRef<{ start: Point; current: Annotation | null } | null>(null);
  const confirming = useRef(false);
  const [loaded, setLoaded] = useState(false);
  const [editing, setEditing] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [current, setCurrent] = useState<Annotation | null>(null);
  const [tool, setTool] = useState<Tool>('rect');
  const [color, setColor] = useState(colors[0]);
  const [width, setWidth] = useState(3);
  const [draft, setDraft] = useState<Extract<Annotation, { type: 'text' }> | null>(null);
  const [exporting, setExporting] = useState(false);
  const [areaSize, setAreaSize] = useState({ width: 1, height: 1 });
  const disabled = busy || exporting;

  useEffect(() => {
    let cancelled = false;
    const base = new Image();
    base.crossOrigin = 'anonymous'; // Dedicated protocol allows only this editor's local origin.
    base.onload = () => {
      if (cancelled) return;
      baseRef.current = base; setLoaded(true); onReady();
    };
    base.onerror = () => { if (!cancelled) onFailure('无法加载截图'); };
    base.src = image;
    return () => { cancelled = true; base.onload = null; base.onerror = null; baseRef.current = null; };
  }, [image, onReady, onFailure]);
  useLayoutEffect(() => {
    const area = areaRef.current;
    if (!area) return;
    const resize = () => setAreaSize({ width: area.clientWidth, height: area.clientHeight });
    resize();
    const observer = new ResizeObserver(resize); observer.observe(area);
    return () => observer.disconnect();
  }, [editing]);

  const natural = editing && selection ? { width: selection.w, height: selection.h }
    : { width: baseRef.current?.naturalWidth || 1, height: baseRef.current?.naturalHeight || 1 };
  const fit = Math.min(areaSize.width / natural.width, areaSize.height / natural.height);
  useLayoutEffect(() => {
    const canvas = canvasRef.current, base = baseRef.current;
    if (!canvas || !base || !loaded) return;
    canvas.width = natural.width; canvas.height = natural.height;
    const ctx = canvas.getContext('2d');
    if (!ctx) { onFailure('无法创建截图画布'); return; }
    if (editing && selection) {
      ctx.drawImage(base, selection.x, selection.y, selection.w, selection.h, 0, 0, canvas.width, canvas.height);
      for (const a of [...annotations, ...(current ? [current] : []), ...(draft ? [draft] : [])]) drawAnnotation(ctx, a, base, selection);
    } else {
      ctx.drawImage(base, 0, 0);
      ctx.fillStyle = 'rgba(0,0,0,0.4)'; ctx.fillRect(0, 0, canvas.width, canvas.height);
      if (selection?.w && selection.h) {
        const s = selection;
        ctx.drawImage(base, s.x, s.y, s.w, s.h, s.x, s.y, s.w, s.h);
        ctx.strokeStyle = '#22c55e'; ctx.lineWidth = 2 / Math.max(fit, 0.01); ctx.strokeRect(s.x, s.y, s.w, s.h);
      }
    }
  }, [loaded, natural.width, natural.height, editing, selection, annotations, current, draft, fit, onFailure]);

  const point = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = event.currentTarget;
    return imagePoint(event.clientX, event.clientY, canvas.getBoundingClientRect(), canvas.width, canvas.height);
  };
  const commitDraft = () => {
    if (draft?.text.trim()) setAnnotations((items) => [...items, draft]);
    setDraft(null);
  };
  const down = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (disabled || event.button !== 0 || !loaded || annotations.length >= 200) return;
    event.preventDefault();
    const p = point(event);
    const stroke = width / Math.max(fit, 0.01);
    if (editing && tool === 'text') {
      commitDraft();
      setDraft({ type: 'text', point: p, text: '', color, width: 20 / Math.max(fit, 0.01) });
      return;
    }
    if (draft) commitDraft();
    event.currentTarget.setPointerCapture(event.pointerId);
    let a: Annotation | null = null;
    if (editing) {
      if (tool === 'arrow') a = { type: 'arrow', start: p, end: p, color, width: stroke };
      else if (tool === 'pencil') a = { type: 'pencil', points: [p], color, width: stroke };
      else if (tool === 'rect' || tool === 'mosaic') a = { type: tool, x: p.x, y: p.y, w: 0, h: 0, color, width: stroke };
    } else setSelection(null);
    dragRef.current = { start: p, current: a }; setCurrent(a);
  };
  const move = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    const p = point(event), a = drag.current;
    if (!editing) setSelection(imageSelection(drag.start, p, natural.width, natural.height));
    else if (a) {
      if (a.type === 'pencil') { if (a.points.length < 10000) drag.current = { ...a, points: [...a.points, p] }; }
      else if (a.type === 'arrow') drag.current = { ...a, end: p };
      else if (a.type === 'rect' || a.type === 'mosaic') drag.current = { ...a, ...imageSelection(drag.start, p, natural.width, natural.height) };
      setCurrent(drag.current);
    }
  };
  const up = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!dragRef.current) return;
    move(event);
    const drag = dragRef.current;
    if (!editing) {
      const selected = imageSelection(drag.start, point(event), natural.width, natural.height);
      if (selected.w >= 2 && selected.h >= 2) { setSelection(selected); setEditing(true); setAnnotations([]); }
    } else if (drag.current) {
      const a = drag.current;
      const valid = a.type === 'arrow' ? Math.hypot(a.end.x - a.start.x, a.end.y - a.start.y) >= 2
        : a.type === 'pencil' ? a.points.length >= 2 : a.type === 'text' || (a.w > 0 && a.h > 0);
      if (valid) setAnnotations((items) => [...items, a]);
    }
    dragRef.current = null; setCurrent(null);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  };
  const confirm = useCallback(async () => {
    if (disabled || confirming.current || dragRef.current || !editing || !selection || !baseRef.current) return;
    confirming.current = true; setExporting(true);
    try {
      // Export from source + committed annotations, never the selection mask or tool UI.
      const output = document.createElement('canvas'); output.width = selection.w; output.height = selection.h;
      const ctx = output.getContext('2d'); if (!ctx) throw new Error('无法导出截图');
      ctx.drawImage(baseRef.current, selection.x, selection.y, selection.w, selection.h, 0, 0, output.width, output.height);
      for (const a of [...annotations, ...(draft?.text.trim() ? [draft] : [])]) drawAnnotation(ctx, a, baseRef.current, selection);
      const blob = await new Promise<Blob>((resolve, reject) => output.toBlob((value) => value ? resolve(value) : reject(new Error('截图编码失败')), 'image/png'));
      if (blob.size > 20 * 1024 * 1024) throw new Error('选区超过20 MiB，请缩小截图范围');
      await onConfirm(blob);
    } catch (error) { onFailure(String(error)); }
    finally { confirming.current = false; setExporting(false); }
  }, [disabled, editing, selection, annotations, draft, onConfirm, onFailure]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229) return;
      if (event.key === 'Escape') { event.preventDefault(); onCancel(); return; }
      if (event.target instanceof HTMLElement && ['TEXTAREA', 'INPUT', 'BUTTON', 'SELECT'].includes(event.target.tagName)) return;
      if (disabled) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') {
        event.preventDefault(); if (draft) setDraft(null); else setAnnotations((items) => items.slice(0, -1));
      }
      if (event.key === 'Enter') { event.preventDefault(); void confirm(); }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [onCancel, disabled, draft, confirm]);

  return <div className="fixed inset-0 bg-neutral-900 text-white select-none" role="dialog" aria-label="截图与标注">
    <div ref={areaRef} className="absolute flex items-center justify-center" style={{ inset: editing ? '12px 16px 112px' : '0' }}>
      <canvas ref={canvasRef} aria-label="拖动选择截图区域或绘制标注" style={{ width: natural.width * fit, height: natural.height * fit, touchAction: 'none', cursor: 'crosshair' }}
        onPointerDown={down} onPointerMove={move} onPointerUp={up}
        onPointerCancel={() => { dragRef.current = null; setCurrent(null); }} />
    </div>
    {!editing && <div className="absolute top-4 inset-x-0 flex justify-center pointer-events-none">
      <div className="flex items-center gap-4 px-4 py-2 rounded-lg bg-black/75 text-sm pointer-events-auto">
        <span>{loaded ? '拖动选择区域 · Esc 取消' : '正在加载截图…'}</span>
        <button disabled={!loaded} onClick={() => { setSelection({ x: 0, y: 0, w: natural.width, h: natural.height }); setEditing(true); }} className="hover:text-primary-400">选择全屏</button>
        <button title="取消截图" onClick={onCancel}><FiX size={18} /></button>
      </div>
    </div>}
    {editing && <div className="absolute bottom-3 inset-x-3 flex flex-col items-center gap-2">
      {draft && <div className="flex gap-2 rounded-lg bg-neutral-800 p-2 border border-neutral-600">
        <textarea autoFocus aria-label="标注文字" placeholder="输入文字，支持换行" maxLength={1000} rows={2} value={draft.text}
          disabled={disabled} onChange={(event) => setDraft({ ...draft, text: event.target.value })}
          className="w-64 resize-none bg-neutral-900 text-white px-2 py-1 text-sm outline-none select-text" />
        <button disabled={disabled} onClick={commitDraft} className="px-2 text-primary-400">添加</button>
      </div>}
      <div className="flex flex-wrap justify-center items-center gap-2 bg-neutral-800 border border-neutral-600 rounded-lg px-3 py-2 shadow-xl">
        {tools.map(({ value, label, Icon }) => <button key={value} title={label} aria-label={label} aria-pressed={tool === value} disabled={disabled}
          onClick={() => { commitDraft(); setTool(value); }} className={`p-2 rounded ${tool === value ? 'bg-primary-600 text-white' : 'hover:bg-neutral-700'} disabled:opacity-40`}><Icon size={18} /></button>)}
        <span className="w-px h-5 bg-neutral-600 mx-1" />
        {colors.map((value) => <button key={value} title={`颜色 ${value}`} aria-label={`颜色 ${value}`} disabled={disabled} onClick={() => setColor(value)}
          className={`w-4 h-4 rounded-full border ${value === color ? 'ring-2 ring-offset-2 ring-offset-neutral-800 ring-primary-400' : 'border-neutral-500'}`} style={{ backgroundColor: value }} />)}
        <select aria-label="线条宽度" value={width} disabled={disabled} onChange={(e) => setWidth(Number(e.target.value))} className="ml-2 bg-neutral-700 text-xs rounded p-1"><option value={2}>细</option><option value={3}>中</option><option value={6}>粗</option></select>
        <button title="撤销 Ctrl+Z" aria-label="撤销" disabled={disabled || (!annotations.length && !draft)} onClick={() => { if (draft) setDraft(null); else setAnnotations((items) => items.slice(0, -1)); }} className="p-2 disabled:opacity-30"><FiRotateCcw /></button>
        <button title="重新选区" aria-label="重新选区" disabled={disabled} onClick={() => { setEditing(false); setSelection(null); setAnnotations([]); setCurrent(null); setDraft(null); }} className="p-2"><FiMaximize /></button>
        <button title="取消 Esc" onClick={onCancel} className="flex items-center gap-1 p-2 text-sm"><FiX />取消</button>
        <button disabled={disabled} onClick={() => void confirm()} className="flex items-center gap-1 px-3 py-2 rounded bg-primary-600 text-sm disabled:opacity-50"><FiCheck />{disabled ? '处理中…' : '发送'}</button>
      </div>
      <p className="text-xs text-neutral-400">{selection?.w} × {selection?.h} 像素 · {tool === 'text' ? '点击图片放置文字' : '拖动添加标注'} · Enter 发送 · Esc 取消</p>
    </div>}
  </div>;
}
