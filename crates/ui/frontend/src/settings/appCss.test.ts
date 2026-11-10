import { describe, expect, it } from 'vitest'

// `?raw` CSS imports come back empty under vitest (CSS is not processed), so read the file with
// Node's `fs`, the same way `sourceTypes.test.ts` does (runtime module name: no `@types/node`).
const fs: { readFileSync(path: URL, encoding: 'utf8'): string } = await import(/* @vite-ignore */ `node:${'fs'}`)
const here = import.meta.url
const css = fs.readFileSync(new URL('./App.css', here), 'utf8')

/*
 * Tailwind v4 declares `@layer theme, base, components, utilities`, and an unlayered rule beats
 * every layered one. An unlayered `.btn { padding }` silently overrode `px-0` on `btn w-7 px-0`
 * and squeezed the Open-folder icon to 4px. So component classes live in `@layer components`,
 * and global defaults that clash with them (`:focus-visible` vs `.field { outline: none }`) in
 * `@layer base`.
 */

/** `src` with every `@layer <name> { … }` block (balanced braces) cut out. */
function withoutLayer(src: string, name: string): string {
  let out = ''
  let i = 0
  for (;;) {
    const start = src.indexOf(`@layer ${name} {`, i)
    if (start === -1) return out + src.slice(i)
    out += src.slice(i, start)
    let j = src.indexOf('{', start)
    let depth = 0
    for (; j < src.length; j++) {
      if (src[j] === '{') depth++
      else if (src[j] === '}' && --depth === 0) break
    }
    i = j + 1
  }
}

/** `src` without comments, so prose that names a selector doesn't count as a rule. */
const stripComments = (src: string) => src.replace(/\/\*[\s\S]*?\*\//g, '')

describe('App.css', () => {
  it('imports tailwind first, so its layer order (components below utilities) applies', () => {
    expect(css.trimStart().startsWith('@import "tailwindcss";')).toBe(true)
  })

  it.each([
    ['.field', /\.field(?![\w-])/],
    ['.field-mono', /\.field-mono(?![\w-])/],
    ['.btn', /\.btn(?![\w-])/],
    ['.btn-accent', /\.btn-accent(?![\w-])/],
    ['.btn-plain', /\.btn-plain(?![\w-])/],
  ])('defines %s only inside @layer components', (cls, selector) => {
    expect(css).toContain(`${cls} {`)
    expect(withoutLayer(stripComments(css), 'components')).not.toMatch(selector)
  })

  it('defines :focus-visible only inside @layer base, below .field and outline-none', () => {
    expect(css).toContain(':focus-visible {')
    expect(withoutLayer(stripComments(css), 'base')).not.toContain(':focus-visible')
  })
})
