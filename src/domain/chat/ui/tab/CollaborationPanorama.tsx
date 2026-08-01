import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type WheelEvent } from 'react';
import {
  Bot,
  CheckCircle2,
  CircleAlert,
  Clock3,
  FileText,
  GitCompare,
  LoaderCircle,
  Minus,
  Plus,
  Scan,
} from 'lucide-react';
import type { WorkspaceRef } from '@/domain/studio';
import type { RuntimeTask } from '../../state/taskRuntimeStore';
import { isTaskActive, presentRuntimeTask, type RuntimeStatusIcon } from './delegationPresentation';
import {
  layoutCollaboration,
  selectCollaborationLineage,
  type CollaborationLayoutNode,
} from './collaborationLayout';

interface Props {
  tasks: RuntimeTask[];
  batchId?: string;
  rootTitle?: string;
  onOpenDocument: (documentId: string, task: RuntimeTask, title?: string) => void;
  onOpenDiff: (ref: WorkspaceRef, task: RuntimeTask) => void;
}

const MIN_ZOOM = 0.45;
const MAX_ZOOM = 1.45;
const ZOOM_STEP = 0.1;

function clampZoom(value: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, Math.round(value * 100) / 100));
}

function StatusGlyph({ kind }: { kind: RuntimeStatusIcon }) {
  const props = { size: 12, strokeWidth: 2, 'aria-hidden': true } as const;
  if (kind === 'completed') return <CheckCircle2 {...props} />;
  if (kind === 'failed' || kind === 'attention' || kind === 'recovery') return <CircleAlert {...props} />;
  if (kind === 'running' || kind === 'starting' || kind === 'materializing') {
    return <LoaderCircle {...props} className="collaboration-panorama-spin" />;
  }
  return <Clock3 {...props} />;
}

function CollaborationNode({
  node,
  rootTitle,
  rootStatus,
  rootActive,
  onOpenDocument,
  onOpenDiff,
}: {
  node: CollaborationLayoutNode;
  rootTitle: string;
  rootStatus: string;
  rootActive: boolean;
  onOpenDocument: Props['onOpenDocument'];
  onOpenDiff: Props['onOpenDiff'];
}) {
  const style = {
    width: node.width,
    height: node.height,
    transform: `translate(${node.x}px, ${node.y}px)`,
  };
  if (!node.task) {
    return (
      <article className="collaboration-panorama-node is-root" style={style} data-panorama-node aria-label={`主 Agent：${rootTitle}`}>
        <header>
          <span className="collaboration-panorama-avatar"><Bot size={15} /></span>
          <span><small>主 Agent</small><strong>Workbench</strong></span>
          <em>主控</em>
        </header>
        <div className="collaboration-panorama-title">{rootTitle}</div>
        <footer className={rootActive ? 'is-running' : 'is-completed'}>
          {rootActive
            ? <LoaderCircle size={12} className="collaboration-panorama-spin" />
            : <CheckCircle2 size={12} />}
          {rootStatus}
        </footer>
      </article>
    );
  }

  const task = node.task;
  const view = presentRuntimeTask(task);
  const identity = view.displayName ? `${view.roleLabel} ${view.displayName}` : view.roleLabel;
  const firstChangedFile = view.changedFiles[0];
  return (
    <article
      className={`collaboration-panorama-node is-${view.statusIcon}`}
      style={style}
      data-panorama-node
      aria-label={`${identity}：${view.documentTitle}，${view.statusLabel}`}
    >
      <header>
        <span className={`collaboration-panorama-avatar is-tone-${view.avatarTone}`}><Bot size={14} /></span>
        <span title={identity}><small>{view.roleLabel}</small><strong>{view.displayName || '未命名'}</strong></span>
        <em title={`执行器：${view.executorLabel}`}>{view.executorLabel}</em>
      </header>
      {view.documentId ? (
        <button
          type="button"
          className="collaboration-panorama-title"
          onClick={() => onOpenDocument(view.documentId!, task, view.documentTitle)}
          title={`阅读：${view.documentTitle}`}
        >
          <FileText size={12} />
          <span>{view.documentTitle}</span>
        </button>
      ) : (
        <div className="collaboration-panorama-title" title={view.documentTitle}>{view.documentTitle}</div>
      )}
      <footer className={`is-${view.statusIcon}`}>
        <StatusGlyph kind={view.statusIcon} />
        <span>{view.statusLabel}</span>
        {firstChangedFile && (
          <button
            type="button"
            onClick={() => onOpenDiff({
              path: firstChangedFile.path,
              kind: 'diff',
              taskId: task.id,
              workspaceRoot: task.cwd,
              reviewRevision: task.reviewRevision,
              inlinePatch: firstChangedFile.patch,
            }, task)}
            title={`审阅 ${view.changedFiles.length} 个改动文件`}
          >
            <GitCompare size={11} />
            审阅 {view.changedFiles.length} 个文件
          </button>
        )}
      </footer>
    </article>
  );
}

export default function CollaborationPanorama({
  tasks,
  batchId,
  rootTitle = '协调协作者完成任务',
  onOpenDocument,
  onOpenDiff,
}: Props) {
  const lineage = useMemo(() => selectCollaborationLineage(tasks, batchId), [batchId, tasks]);
  const layout = useMemo(() => layoutCollaboration(lineage), [lineage]);
  const viewportRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; scrollLeft: number; scrollTop: number }>();
  const [zoom, setZoom] = useState(1);
  const [dragging, setDragging] = useState(false);
  const activeCount = lineage.filter(isTaskActive).length;
  const completedCount = lineage.length - activeCount;
  const rootStatus = activeCount > 0
    ? `协调中 · ${activeCount} 位进行中`
    : `${completedCount}/${lineage.length} 位协作者已完成`;

  const fit = () => {
    const viewport = viewportRef.current;
    if (!viewport || viewport.clientWidth <= 0 || viewport.clientHeight <= 0) return;
    const nextZoom = clampZoom(Math.min(
      (viewport.clientWidth - 48) / layout.width,
      (viewport.clientHeight - 48) / layout.height,
      1,
    ));
    setZoom(nextZoom);
    window.requestAnimationFrame(() => {
      viewport.scrollLeft = Math.max(0, (layout.width * nextZoom - viewport.clientWidth) / 2);
      viewport.scrollTop = 0;
    });
  };

  useEffect(() => {
    fit();
    // A changed graph should refit; manual zoom remains stable between status-only updates.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout.width, layout.height]);

  const beginPan = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const target = event.target as Element;
    if (target.closest('[data-panorama-node], .collaboration-panorama-toolbar')) return;
    const viewport = viewportRef.current;
    if (!viewport) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      scrollLeft: viewport.scrollLeft,
      scrollTop: viewport.scrollTop,
    };
    setDragging(true);
  };

  const movePan = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    const viewport = viewportRef.current;
    if (!drag || !viewport || drag.pointerId !== event.pointerId) return;
    viewport.scrollLeft = drag.scrollLeft - (event.clientX - drag.x);
    viewport.scrollTop = drag.scrollTop - (event.clientY - drag.y);
  };

  const endPan = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = undefined;
    setDragging(false);
  };

  const zoomFromWheel = (event: WheelEvent<HTMLDivElement>) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    setZoom(current => clampZoom(current + (event.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP)));
  };

  return (
    <section className="collaboration-panorama" aria-label="协作全景图">
      <div className="collaboration-panorama-toolbar" role="toolbar" aria-label="全景图缩放">
        <span>{lineage.length} 位协作者</span>
        <button type="button" onClick={() => setZoom(value => clampZoom(value - ZOOM_STEP))} disabled={zoom <= MIN_ZOOM} aria-label="缩小全景图"><Minus size={14} /></button>
        <output aria-label="当前缩放比例">{Math.round(zoom * 100)}%</output>
        <button type="button" onClick={() => setZoom(value => clampZoom(value + ZOOM_STEP))} disabled={zoom >= MAX_ZOOM} aria-label="放大全景图"><Plus size={14} /></button>
        <button type="button" onClick={fit} aria-label="适应全景图"><Scan size={14} /></button>
      </div>
      <div
        ref={viewportRef}
        className={`collaboration-panorama-viewport ${dragging ? 'is-dragging' : ''}`}
        onPointerDown={beginPan}
        onPointerMove={movePan}
        onPointerUp={endPan}
        onPointerCancel={endPan}
        onWheel={zoomFromWheel}
      >
        <div
          className="collaboration-panorama-canvas"
          style={{ width: layout.width * zoom, height: layout.height * zoom }}
        >
          <div
            className="collaboration-panorama-surface"
            style={{ width: layout.width, height: layout.height, transform: `scale(${zoom})` }}
          >
            <svg width={layout.width} height={layout.height} aria-hidden="true">
              {layout.edges.map(edge => <path key={edge.id} d={edge.path} />)}
            </svg>
            {layout.nodes.map(node => (
              <CollaborationNode
                key={node.id}
                node={node}
                rootTitle={rootTitle}
                rootStatus={rootStatus}
                rootActive={activeCount > 0}
                onOpenDocument={onOpenDocument}
                onOpenDiff={onOpenDiff}
              />
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
