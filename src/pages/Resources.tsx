import { Search } from 'lucide-react';
import { useState } from 'react';
import { ResourceCard } from '../components/ResourceCard';
import type { LabResource } from '../types';

export function Resources({
  resources,
  busy,
  onOpen,
}: {
  resources: LabResource[];
  busy: boolean;
  onOpen: (id: string) => void;
}) {
  const [query, setQuery] = useState('');
  const filtered = resources.filter((resource) =>
    `${resource.name} ${resource.address}`
      .toLowerCase()
      .includes(query.trim().toLowerCase()),
  );
  return (
    <>
      {resources.length > 6 && (
        <label className="search">
          <Search size={16} aria-hidden="true" />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索资源"
            aria-label="搜索资源"
          />
        </label>
      )}
      {filtered.length ? (
        <div className="resource-list">
          {filtered.map((resource) => (
            <ResourceCard
              key={resource.id}
              resource={resource}
              disabled={busy}
              onOpen={() => onOpen(resource.id)}
            />
          ))}
        </div>
      ) : (
        <p className="empty-state">{query ? '无匹配资源' : '暂无资源'}</p>
      )}
    </>
  );
}
