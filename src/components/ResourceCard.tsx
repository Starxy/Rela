import { ArrowUpRight, Database, NotebookPen, Server } from 'lucide-react';
import type { LabResource } from '../types';

const labels = {
  unknown: '未检测',
  reachable: '可访问',
  unreachable: '未响应',
};
const icons = { ssh: Server, web: NotebookPen, nas: Database };

export function ResourceCard({
  resource,
  disabled,
  onOpen,
}: {
  resource: LabResource;
  disabled: boolean;
  onOpen: () => void;
}) {
  const Icon = icons[resource.kind];
  return (
    <button
      className="resource-row"
      onClick={onOpen}
      disabled={disabled || resource.availability !== 'reachable'}
      aria-label={`打开 ${resource.name}`}
    >
      <span className="entry-icon">
        <Icon size={18} aria-hidden="true" />
      </span>
      <span className="resource-info">
        <strong>{resource.name}</strong>
        <span className="mono">{resource.address}</span>
      </span>
      <span className={`resource-state ${resource.availability}`}>
        <i aria-hidden="true" />
        {labels[resource.availability]}
      </span>
      <ArrowUpRight className="resource-arrow" size={14} aria-hidden="true" />
    </button>
  );
}
