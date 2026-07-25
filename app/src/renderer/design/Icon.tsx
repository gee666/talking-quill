const PATHS = {
  dashboard: 'M2.5 2.5h4.5v4.5H2.5zM9 2.5h4.5v4.5H9zM2.5 9h4.5v4.5H2.5zM9 9h4.5v4.5H9z',
  settings:
    'M8 10a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM13 8a5 5 0 0 0-.1-.9l1.2-.9-1.3-2.2-1.4.5a5 5 0 0 0-1.5-.9L9.7 2H6.3l-.2 1.5a5 5 0 0 0-1.5.9l-1.4-.5-1.3 2.2 1.2.9a5 5 0 0 0 0 1.8l-1.2.9 1.3 2.2 1.4-.5a5 5 0 0 0 1.5.9l.2 1.5h3.4l.2-1.5a5 5 0 0 0 1.5-.9l1.4.5 1.3-2.2-1.2-.9c.07-.3.1-.6.1-.9Z',
  info: 'M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12ZM8 7.5V11M8 5.2v.3',
  general: 'M3 4h10M3 8h10M3 12h6',
  profiles: 'M2.5 4.5h11v7h-11zM4.5 6.5h1M7 6.5h2M11 6.5h.5M4.5 9.5h7',
  recording:
    'M8 2.5a2 2 0 0 1 2 2v3.5a2 2 0 1 1-4 0V4.5a2 2 0 0 1 2-2ZM4 7.5v.5a4 4 0 0 0 8 0v-.5M8 12v2',
  model: 'M8 2 3 4.8v6.4L8 14l5-2.8V4.8L8 2ZM3 4.8 8 7.6l5-2.8M8 7.6V14',
  privacy: 'M8 2 3.5 4v4c0 3 2 5 4.5 6 2.5-1 4.5-3 4.5-6V4L8 2ZM6.2 8l1.3 1.3L10 6.8',
  smart: 'M8 2.2l1.5 3.6 3.8.3-2.9 2.5.9 3.7L8 10.4l-3.3 1.9.9-3.7-2.9-2.5 3.8-.3L8 2.2Z',
  commands: 'M5.5 2.5h5M3 5.5h10v8H3zM5.5 8.5h5M5.5 11h3',
  vocabulary: 'M3 3.5h7a2 2 0 0 1 2 2v7H5a2 2 0 0 1-2-2v-7ZM12 12.5h1M5 6h4M5 8.5h4',
  sun: 'M8 10.8a2.8 2.8 0 1 0 0-5.6 2.8 2.8 0 0 0 0 5.6ZM8 1.6v1.4M8 13v1.4M2.3 8h1.4M12.3 8h1.4M3.9 3.9l1 1M11.1 11.1l1 1M12.1 3.9l-1 1M4.9 11.1l-1 1',
  moon: 'M13 9.6A5.5 5.5 0 0 1 6.4 3 5.6 5.6 0 1 0 13 9.6Z',
  minimize: 'M3.5 8h9',
  maximize: 'M3.5 3.5h9v9h-9z',
  restore: 'M5.5 5.5h7v7h-7zM3.5 10.5v-7h7',
  close: 'M4 4l8 8M12 4l-8 8',
  chevron: 'M6 4l4 4-4 4',
  check: 'M3.5 8.5l3 3 6-6.5',
  search: 'M7.3 12.1a4.8 4.8 0 1 0 0-9.6 4.8 4.8 0 0 0 0 9.6ZM10.8 10.8l2.7 2.7',
  trash: 'M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5M6.8 7v3.5M9.2 7v3.5',
  copy: 'M5.5 5.5h7v7h-7zM3.5 10.5v-7h7',
  plus: 'M8 3.5v9M3.5 8h9',
  external: 'M9 3.5h3.5V7M12.5 3.5 7.5 8.5M11 9.5v3h-8v-8h3',
} as const;

export type IconName = keyof typeof PATHS;

export interface IconProps {
  readonly name: IconName;
  readonly size?: number;
  readonly className?: string;
}

export function Icon({ name, size = 14, className = '' }: IconProps) {
  return (
    <svg
      className={`me-icon ${className}`.trim()}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
