// ============================================================================
// ScreenshotEditor - full-screen screenshot region selector + annotation editor
//
// Flow (driven by ChatPanel):
//   1. ChatPanel calls `screenshot.capture` (C++ hides the window, captures the
//      current monitor, returns a base64 PNG, then re-shows the window maximised).
//   2. ChatPanel renders <ScreenshotEditor> with that image.
//   3. User drags to select a region, then annotates (rect / arrow / pencil / mosaic).
//   4. Confirm -> onConfirm(dataUrl) (ChatPanel sends it as an image message).
//      Save    -> writes the image to a file chosen via the native save dialog.
//      Cancel  -> onCancel() (ChatPanel restores the window).
// ============================================================================

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { invoke } from '../services/bridge';

type Tool = 'rect' | 'arrow' | 'pencil' | 'mosaic';

interface RectAnno  { type: 'rect';  x: number; y: number; w: number; h: number; color: string; width: number; }
interface ArrowAnno { type: 'arrow'; x1: number; y1: number; x2: number; y2: number; color: string; width: number; }
interface PencilAnno{ type: 'pencil'; points: { x: number; y: number }[]; color: string; width: number; }
interface MosaicAnno{ type: 'mosaic';x: number; y: number; w: number; h: number; block: number; }
type Annotation = RectAnno | ArrowAnno | PencilAnno | MosaicAnno;

interface ScreenshotEditorProps {
  image: string;                 // data URL of the captured monitor
  screenCount?: number;          // number of monitors
  onCancel: () => void;
  onConfirm: (dataUrl: string) => void;
}

const PRESET_COLORS = ['#ff3b30', '#ff9500', '#ffcc00', '#34c759', '#007aff', '#ffffff', '#000000'];

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = reject;
    r.readAsDataURL(blob);
  });
}

// ---- SVG Icons for tools ----
function RectIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="1" />
    </svg>
  );
}

function ArrowIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="20" x2="20" y2="4" />
      <polyline points="9 4 20 4 20 15" />
    </svg>
  );
}

function PencilIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
    </svg>
  );
}

function MosaicIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill={color} stroke="none">
      <rect x="2" y="2" width="6" height="6" />
      <rect x="9" y="2" width="6" height="6" opacity="0.3" />
      <rect x="16" y="2" width="6" height="6" />
      <rect x="2" y="9" width="6" height="6" opacity="0.3" />
      <rect x="9" y="9" width="6" height="6" />
      <rect x="16" y="9" width="6" height="6" opacity="0.3" />
      <rect x="2" y="16" width="6" height="6" />
      <rect x="9" y="16" width="6" height="6" opacity="0.3" />
      <rect x="16" y="16" width="6" height="6" />
    </svg>
  );
}

function UndoIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="7 4 3 8 7 12" />
      <path d="M3 8h10a5 5 0 0 1 0 10h-2" />
    </svg>
  );
}

function RedoSelectIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" strokeDasharray="2 3">
      <rect x="3" y="3" width="18" height="18" rx="1" />
    </svg>
  );
}

function SaveIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 3v12" />
      <polyline points="7 10 12 15 17 10" />
      <line x1="3" y1="20" x2="21" y2="20" />
    </svg>
  );
}

function CheckIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="20 6 9 17 4 12" />
    </svg>
  );
}

function CloseIcon({ size = 18, color = 'currentColor' }: { size?: number; color?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="18" y1="6" x2="6" y2="18" />
      <line x1="6" y1="6" x2="18" y2="18" />
    </svg>
  );
}

export default function ScreenshotEditor({ image, screenCount, onCancel, onConfirm }: ScreenshotEditorProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const baseCanvasRef = useRef<HTMLCanvasElement | null>(null);

  const [loaded, setLoaded] = useState(false);
  const [phase, setPhase] = useState<'select' | 'edit'>('select');
  const [sel, setSel] = useState<{ x: number; y: number; w: number; h: number } | null>(null);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [current, setCurrent] = useState<Annotation | null>(null);
  const [tool, setTool] = useState<Tool | null>('rect');
  const [color, setColor] = useState('#ff3b30');
  const [strokeWidth, setStrokeWidth] = useState(3);

  const dragStartRef = useRef<{ x: number; y: number } | null>(null);

  // ---- Load the captured image into an offscreen base canvas ----
  useEffect(() => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      c.getContext('2d')!.drawImage(img, 0, 0);
      baseCanvasRef.current = c;
      setLoaded(true);
    };
    img.src = image;
  }, [image]);

  // ---- Natural (canvas pixel) size for the current phase ----
  const getNatural = useCallback((): { w: number; h: number } => {
    if (phase === 'edit' && sel) return { w: Math.max(1, Math.round(sel.w)), h: Math.max(1, Math.round(sel.h)) };
    const img = baseCanvasRef.current;
    return img ? { w: img.width, h: img.height } : { w: 1, h: 1 };
  }, [phase, sel]);

  // ---- Convert a mouse event to canvas-natural coordinates ----
  const toNatural = useCallback((e: React.MouseEvent): { x: number; y: number } => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    return {
      x: ((e.clientX - rect.left) / rect.width) * canvas.width,
      y: ((e.clientY - rect.top) / rect.height) * canvas.height,
    };
  }, []);

  // ---- Draw a single annotation ----
  const drawAnno = useCallback((ctx: CanvasRenderingContext2D, a: Annotation, scale: number, s: { x: number; y: number }, base: HTMLCanvasElement) => {
    if (a.type === 'rect') {
      ctx.strokeStyle = a.color;
      ctx.lineWidth = Math.max(1, a.width * scale);
      ctx.strokeRect(a.x, a.y, a.w, a.h);
    } else if (a.type === 'arrow') {
      ctx.strokeStyle = a.color;
      ctx.lineWidth = Math.max(1, a.width * scale);
      ctx.beginPath();
      ctx.moveTo(a.x1, a.y1);
      ctx.lineTo(a.x2, a.y2);
      ctx.stroke();
      const ang = Math.atan2(a.y2 - a.y1, a.x2 - a.x1);
      const head = Math.max(12, a.width * scale * 3.5);
      const a1 = ang + Math.PI * 0.85;
      const a2 = ang - Math.PI * 0.85;
      ctx.beginPath();
      ctx.moveTo(a.x2, a.y2);
      ctx.lineTo(a.x2 + head * Math.cos(a1), a.y2 + head * Math.sin(a1));
      ctx.moveTo(a.x2, a.y2);
      ctx.lineTo(a.x2 + head * Math.cos(a2), a.y2 + head * Math.sin(a2));
      ctx.stroke();
    } else if (a.type === 'pencil') {
      if (a.points.length < 2) return;
      ctx.strokeStyle = a.color;
      ctx.lineWidth = Math.max(1, a.width * scale);
      ctx.lineCap = 'round';
      ctx.lineJoin = 'round';
      ctx.beginPath();
      ctx.moveTo(a.points[0].x, a.points[0].y);
      for (let i = 1; i < a.points.length; i++) {
        ctx.lineTo(a.points[i].x, a.points[i].y);
      }
      ctx.stroke();
    } else if (a.type === 'mosaic') {
      const bw = Math.max(4, Math.round(a.block * scale));
      const tmpW = Math.max(1, Math.floor(a.w / bw));
      const tmpH = Math.max(1, Math.floor(a.h / bw));
      const tmp = document.createElement('canvas');
      tmp.width = tmpW;
      tmp.height = tmpH;
      const tctx = tmp.getContext('2d')!;
      tctx.imageSmoothingEnabled = false;
      tctx.drawImage(base, s.x + a.x, s.y + a.y, a.w, a.h, 0, 0, tmpW, tmpH);
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(tmp, 0, 0, tmpW, tmpH, a.x, a.y, a.w, a.h);
      ctx.imageSmoothingEnabled = true;
    }
  }, []);

  // ---- Render the canvas ----
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const base = baseCanvasRef.current;
    if (!canvas || !base) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const rect = canvas.getBoundingClientRect();
    const scale = canvas.width / (rect.width || canvas.width);

    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (phase === 'select') {
      ctx.drawImage(base, 0, 0, canvas.width, canvas.height);
      if (sel && sel.w > 0 && sel.h > 0) {
        ctx.fillStyle = 'rgba(0,0,0,0.45)';
        const { x, y, w, h } = sel;
        ctx.fillRect(0, 0, canvas.width, y);
        ctx.fillRect(0, y + h, canvas.width, canvas.height - y - h);
        ctx.fillRect(0, y, x, h);
        ctx.fillRect(x + w, y, canvas.width - x - w, h);
        ctx.strokeStyle = '#1e90ff';
        ctx.lineWidth = Math.max(1, 2 * scale);
        ctx.strokeRect(x, y, w, h);
      }
    } else if (sel) {
      const s = sel;
      ctx.drawImage(base, s.x, s.y, s.w, s.h, 0, 0, canvas.width, canvas.height);
      const list = current ? [...annotations, current] : annotations;
      for (const a of list) drawAnno(ctx, a, scale, s, base);
    }
  }, [phase, sel, annotations, current, drawAnno]);

  // ---- Layout: size the canvas + wrapper, then draw ----
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    const wrap = wrapRef.current;
    const base = baseCanvasRef.current;
    if (!canvas || !container || !wrap || !base || !loaded) return;

    const nat = getNatural();
    canvas.width = nat.w;
    canvas.height = nat.h;

    const cw = container.clientWidth;
    const ch = container.clientHeight;
    const ar = nat.w / nat.h;
    let fw = cw;
    let fh = cw / ar;
    if (fh > ch) { fh = ch; fw = ch * ar; }
    wrap.style.width = Math.floor(fw) + 'px';
    wrap.style.height = Math.floor(fh) + 'px';
    canvas.style.width = '100%';
    canvas.style.height = '100%';

    draw();
  }, [loaded, phase, sel, annotations, current, color, strokeWidth, getNatural, draw]);

  // Re-fit on window resize.
  useEffect(() => {
    const onResize = () => {
      const canvas = canvasRef.current;
      const container = containerRef.current;
      const wrap = wrapRef.current;
      const base = baseCanvasRef.current;
      if (!canvas || !container || !wrap || !base || !loaded) return;
      const nat = getNatural();
      const cw = container.clientWidth;
      const ch = container.clientHeight;
      const ar = nat.w / nat.h;
      let fw = cw;
      let fh = cw / ar;
      if (fh > ch) { fh = ch; fw = ch * ar; }
      wrap.style.width = Math.floor(fw) + 'px';
      wrap.style.height = Math.floor(fh) + 'px';
    };
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [loaded, phase, sel, getNatural]);

  // ---- Mouse handling ----
  const onMouseDown = (e: React.MouseEvent) => {
    const p = toNatural(e);
    if (phase === 'select') {
      dragStartRef.current = p;
      setSel(null);
    } else if (tool) {
      dragStartRef.current = p;
      if (tool === 'pencil') {
        setCurrent({ type: 'pencil', points: [p], color, width: strokeWidth });
      } else if (tool === 'rect') {
        setCurrent({ type: 'rect', x: p.x, y: p.y, w: 0, h: 0, color, width: strokeWidth });
      } else if (tool === 'arrow') {
        setCurrent({ type: 'arrow', x1: p.x, y1: p.y, x2: p.x, y2: p.y, color, width: strokeWidth });
      } else if (tool === 'mosaic') {
        setCurrent({ type: 'mosaic', x: p.x, y: p.y, w: 0, h: 0, block: Math.max(8, strokeWidth * 4) });
      }
    }
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const start = dragStartRef.current;
    if (!start) return;
    const p = toNatural(e);
    if (phase === 'select') {
      setSel({
        x: Math.min(start.x, p.x),
        y: Math.min(start.y, p.y),
        w: Math.abs(p.x - start.x),
        h: Math.abs(p.y - start.y),
      });
    } else if (current) {
      if (current.type === 'pencil') {
        setCurrent({ ...current, points: [...current.points, p] });
      } else if (current.type === 'rect') {
        setCurrent({ ...current, x: Math.min(start.x, p.x), y: Math.min(start.y, p.y), w: Math.abs(p.x - start.x), h: Math.abs(p.y - start.y) });
      } else if (current.type === 'mosaic') {
        setCurrent({ ...current, x: Math.min(start.x, p.x), y: Math.min(start.y, p.y), w: Math.abs(p.x - start.x), h: Math.abs(p.y - start.y) });
      } else if (current.type === 'arrow') {
        setCurrent({ ...current, x2: p.x, y2: p.y });
      }
    }
  };

  const onMouseUp = () => {
    const start = dragStartRef.current;
    dragStartRef.current = null;
    if (phase === 'select') {
      if (sel && sel.w > 5 && sel.h > 5) {
        setPhase('edit');
        setAnnotations([]);
      }
    } else if (current) {
      const c = current;
      let tiny = false;
      if (c.type === 'rect' || c.type === 'mosaic') tiny = c.w < 3 && c.h < 3;
      else if (c.type === 'arrow') tiny = Math.hypot((c as ArrowAnno).x2 - (c as ArrowAnno).x1, (c as ArrowAnno).y2 - (c as ArrowAnno).y1) < 3;
      else if (c.type === 'pencil') tiny = c.points.length < 3;
      if (!tiny) setAnnotations((a) => [...a, c]);
      setCurrent(null);
    }
    void start;
  };

  // ---- Confirm / Save / Cancel ----
  const exportCanvas = useCallback((): Promise<string | null> => {
    const canvas = canvasRef.current;
    if (!canvas) return Promise.resolve(null);
    return new Promise((resolve) => {
      canvas.toBlob((blob) => {
        if (!blob) { resolve(null); return; }
        blobToDataUrl(blob).then(resolve);
      }, 'image/png');
    });
  }, []);

  const handleConfirm = async () => {
    const dataUrl = await exportCanvas();
    if (dataUrl) onConfirm(dataUrl);
  };

  const handleSave = async () => {
    const dataUrl = await exportCanvas();
    if (!dataUrl) return;
    const base64 = dataUrl.split(',')[1];
    try {
      const now = new Date();
      const ts = now.getFullYear().toString() +
        String(now.getMonth() + 1).padStart(2, '0') +
        String(now.getDate()).padStart(2, '0') +
        String(now.getHours()).padStart(2, '0') +
        String(now.getMinutes()).padStart(2, '0') +
        String(now.getSeconds()).padStart(2, '0');
      const res: any = await invoke('dialog.save', { title: '保存截图', default_name: `Beixin_${ts}_screenshot.png` });
      if (res && res.success && res.path) {
        const sres: any = await invoke('file.save_data', { data: base64, path: res.path });
        if (!sres || !sres.success) alert('保存失败: ' + ((sres && sres.error) || '未知错误'));
      }
    } catch (err) {
      alert('保存失败: ' + err);
    }
  };

  // Esc cancels.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel();
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [onCancel]);

  const isEditing = phase === 'edit';

  const toolItems: { key: Tool; icon: React.ReactNode; title: string }[] = [
    { key: 'rect', icon: <RectIcon />, title: '矩形' },
    { key: 'arrow', icon: <ArrowIcon />, title: '箭头' },
    { key: 'pencil', icon: <PencilIcon />, title: '铅笔' },
    { key: 'mosaic', icon: <MosaicIcon />, title: '马赛克' },
  ];

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 99999,
        background: '#1b1b1b',
        display: 'flex',
        flexDirection: 'column',
        userSelect: 'none',
        fontFamily: 'system-ui, sans-serif',
      }}
    >
      {/* Canvas area */}
      <div
        ref={containerRef}
        style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', position: 'relative', overflow: 'hidden' }}
      >
        <div ref={wrapRef} style={{ position: 'relative', lineHeight: 0 }}>
          <canvas
            ref={canvasRef}
            style={{ display: 'block', cursor: isEditing && tool ? 'crosshair' : 'default', touchAction: 'none' }}
            onMouseDown={onMouseDown}
            onMouseMove={onMouseMove}
            onMouseUp={onMouseUp}
            onMouseLeave={onMouseUp}
            onDragStart={(e) => e.preventDefault()}
          />
        </div>
        {screenCount && screenCount > 1 && (
          <div style={{ position: 'absolute', bottom: 12, left: 12, color: '#aaa', fontSize: 12, background: 'rgba(0,0,0,0.4)', padding: '4px 8px', borderRadius: 4 }}>
            已截取当前屏幕（共 {screenCount} 屏）
          </div>
        )}
      </div>

      {/* Bottom toolbar */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          padding: '6px 12px',
          background: '#404040',
          color: '#eee',
          borderTop: '1px solid #555',
        }}
      >
        {!isEditing && (
          <span style={{ fontWeight: 500, fontSize: 13, opacity: 0.9 }}>拖动鼠标选择要截取的屏幕区域</span>
        )}
        {isEditing && (
          <>
            {/* Tool buttons */}
            <div style={{ display: 'flex', gap: 2, background: '#222', borderRadius: 6, padding: 2 }}>
              {toolItems.map(({ key, icon, title }) => (
                <button
                  key={key}
                  onClick={() => setTool(key)}
                  title={title}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    width: 32,
                    height: 32,
                    borderRadius: 5,
                    cursor: 'pointer',
                    border: 'none',
                    background: tool === key ? '#1e90ff' : 'transparent',
                    color: tool === key ? '#fff' : '#bbb',
                    transition: 'all 0.15s',
                  }}
                >
                  {icon}
                </button>
              ))}
            </div>

            <div style={{ width: 1, height: 24, background: '#444', margin: '0 4px' }} />

            {/* Color picker */}
            <div style={{ display: 'flex', gap: 4, alignItems: 'center' }}>
              {PRESET_COLORS.map((c) => (
                <button
                  key={c}
                  onClick={() => setColor(c)}
                  title={c}
                  style={{
                    width: 18,
                    height: 18,
                    borderRadius: '50%',
                    background: c,
                    cursor: 'pointer',
                    border: color === c ? '2px solid #fff' : '1.5px solid #666',
                    boxShadow: color === c ? '0 0 4px rgba(255,255,255,0.4)' : 'none',
                    transition: 'all 0.15s',
                    padding: 0,
                  }}
                />
              ))}
            </div>

            <div style={{ width: 1, height: 24, background: '#444', margin: '0 4px' }} />

            {/* Stroke width */}
            <label style={{ fontSize: 11, opacity: 0.7, marginRight: 2 }}>粗细</label>
            <input
              type="range"
              min={1}
              max={12}
              value={strokeWidth}
              onChange={(e) => setStrokeWidth(Number(e.target.value))}
              style={{ width: 60, accentColor: '#1e90ff' }}
            />

            <div style={{ width: 1, height: 24, background: '#444', margin: '0 4px' }} />

            {/* Action buttons */}
            <button
              onClick={() => setAnnotations((a) => a.slice(0, -1))}
              disabled={annotations.length === 0}
              title="撤销"
              style={{
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                width: 32, height: 32, borderRadius: 5, border: 'none',
                background: 'transparent', color: annotations.length ? '#bbb' : '#555',
                cursor: annotations.length ? 'pointer' : 'default',
                transition: 'all 0.15s',
              }}
            >
              <UndoIcon />
            </button>
            <button
              onClick={() => { setPhase('select'); setSel(null); setAnnotations([]); setCurrent(null); }}
              title="重新选择"
              style={{
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                width: 32, height: 32, borderRadius: 5, border: 'none',
                background: 'transparent', color: '#bbb', cursor: 'pointer',
                transition: 'all 0.15s',
              }}
            >
              <RedoSelectIcon />
            </button>
          </>
        )}

        <div style={{ flex: 1 }} />

        {/* Right-side actions */}
        <button
          onClick={handleSave}
          disabled={!isEditing}
          title="保存"
          style={{
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            width: 32, height: 32, borderRadius: 5, border: '1px solid #666',
            background: '#4a4a4a', color: isEditing ? '#ddd' : '#666',
            cursor: isEditing ? 'pointer' : 'default',
            transition: 'all 0.15s',
          }}
        >
          <SaveIcon />
        </button>
        <button
          onClick={onCancel}
          title="取消"
          style={{
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            width: 32, height: 32, borderRadius: 5,
            border: '1px solid #a33', background: '#5a1a1a',
            color: '#f44', cursor: 'pointer',
            transition: 'all 0.15s',
          }}
        >
          <CloseIcon />
        </button>
        <button
          onClick={handleConfirm}
          disabled={!isEditing}
          title="确定"
          style={{
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            width: 32, height: 32, borderRadius: 5,
            border: isEditing ? '1px solid #2a2' : '1px solid #353',
            background: isEditing ? '#1a4a1a' : '#2a3a2a',
            color: isEditing ? '#4f4' : '#353',
            cursor: isEditing ? 'pointer' : 'default',
            transition: 'all 0.15s',
          }}
        >
          <CheckIcon />
        </button>
      </div>
    </div>
  );
}
