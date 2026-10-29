export interface Point { x: number; y: number }
export interface Selection { x: number; y: number; w: number; h: number }

/** CSS coordinates are relative to the displayed canvas, never the desktop origin. */
export function imagePoint(clientX: number, clientY: number,
  rect: { left: number; top: number; width: number; height: number }, width: number, height: number): Point {
  return {
    x: Math.max(0, Math.min(width, (clientX - rect.left) * width / Math.max(1, rect.width))),
    y: Math.max(0, Math.min(height, (clientY - rect.top) * height / Math.max(1, rect.height))),
  };
}

/** Keep the crop inside the captured monitor, including reverse and edge drags. */
export function imageSelection(start: Point, end: Point, width: number, height: number): Selection {
  const x = Math.max(0, Math.min(width, Math.floor(Math.min(start.x, end.x))));
  const y = Math.max(0, Math.min(height, Math.floor(Math.min(start.y, end.y))));
  const right = Math.max(x, Math.min(width, Math.ceil(Math.max(start.x, end.x))));
  const bottom = Math.max(y, Math.min(height, Math.ceil(Math.max(start.y, end.y))));
  return { x, y, w: right - x, h: bottom - y };
}
