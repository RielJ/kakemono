/**
 * Stroke icons (lucide geometry, 24×24 viewBox, 2 px stroke), inline so the picker needs no
 * icon package. Decorative: the element they sit in carries the label.
 */
const PATHS = {
  gripVertical: ['M9 12h.01', 'M9 5h.01', 'M9 19h.01', 'M15 12h.01', 'M15 5h.01', 'M15 19h.01'],
  gripHorizontal: ['M12 9h.01', 'M19 9h.01', 'M5 9h.01', 'M12 15h.01', 'M19 15h.01', 'M5 15h.01'],
  check: ['M20 6 9 17l-5-5'],
  image: [
    'M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
    'M10 9a1 1 0 1 1-2 0 1 1 0 0 1 2 0z',
    'm21 15-3.09-3.09a2 2 0 0 0-2.82 0L6 21',
  ],
  // Picker tabs (`../tabs.ts`).
  cloud: ['M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z'],
  download: ['M12 15V3', 'M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4', 'm7 10 5 5 5-5'],
  folder: [
    'M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z',
  ],
  star: ['M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z'],
  archive: [
    'M3 3h18a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z',
    'M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8',
    'M10 12h4',
  ],
} as const

export type IconName = keyof typeof PATHS

export default function Icon({ name, className }: { name: IconName; className?: string }) {
  // Grip dots are zero-length paths: a thicker round cap draws them as dots.
  const dots = name === 'gripVertical' || name === 'gripHorizontal'
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={dots ? 3 : 2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name].map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  )
}
