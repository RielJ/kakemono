/**
 * Stroke icons (lucide geometry, 24×24 viewBox, 2 px stroke), inline so the picker needs no
 * icon package. Decorative: the element they sit in carries the label.
 */
const PATHS = {
  gripVertical: [
    'M9 12h.01',
    'M9 5h.01',
    'M9 19h.01',
    'M15 12h.01',
    'M15 5h.01',
    'M15 19h.01',
  ],
  gripHorizontal: ['M12 9h.01', 'M19 9h.01', 'M5 9h.01', 'M12 15h.01', 'M19 15h.01', 'M5 15h.01'],
  check: ['M20 6 9 17l-5-5'],
  image: [
    'M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
    'M10 9a1 1 0 1 1-2 0 1 1 0 0 1 2 0z',
    'm21 15-3.09-3.09a2 2 0 0 0-2.82 0L6 21',
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
