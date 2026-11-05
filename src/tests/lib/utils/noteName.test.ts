import { describe, it, expect } from 'vitest'
import {
  hasNoteExtension,
  sanitizeNoteName,
  toNoteFileName,
  withNoteExtension,
} from '../../../lib/utils/noteName'

describe('withNoteExtension', () => {
  it('adds .md to a plain name', () => {
    expect(withNoteExtension('shopping')).toBe('shopping.md')
  })

  it('keeps a note extension that is already there', () => {
    expect(withNoteExtension('shopping.md')).toBe('shopping.md')
    expect(withNoteExtension('shopping.txt')).toBe('shopping.txt')
    expect(withNoteExtension('shopping.markdown')).toBe('shopping.markdown')
  })

  it('adds .md to a name that merely contains a dot', () => {
    expect(withNoteExtension('v1.2 notes')).toBe('v1.2 notes.md')
    expect(withNoteExtension('report.final')).toBe('report.final.md')
    expect(withNoteExtension('a.mdx')).toBe('a.mdx.md')
  })

  it('treats a differently-cased extension as not a note extension', () => {
    // The backend only indexes lowercase extensions.
    expect(hasNoteExtension('NOTE.MD')).toBe(false)
    expect(withNoteExtension('NOTE.MD')).toBe('NOTE.MD.md')
  })
})

describe('sanitizeNoteName', () => {
  it('leaves an ordinary name alone', () => {
    expect(sanitizeNoteName('meeting notes')).toBe('meeting notes')
    expect(sanitizeNoteName('2026-11-05 standup')).toBe('2026-11-05 standup')
  })

  it('turns a colon followed by a space into a dash', () => {
    expect(sanitizeNoteName('meeting: acme')).toBe('meeting - acme')
  })

  it('replaces other forbidden characters with a dash', () => {
    expect(sanitizeNoteName('10:30 call')).toBe('10-30 call')
    expect(sanitizeNoteName('what? why*')).toBe('what- why-')
    expect(sanitizeNoteName('a<b>c|d"e')).toBe('a-b-c-d-e')
    expect(sanitizeNoteName('back\\slash')).toBe('back-slash')
  })

  it('replaces control characters', () => {
    expect(sanitizeNoteName('tab\there')).toBe('tab-here')
  })

  it('drops trailing dots and spaces and leading dots', () => {
    expect(sanitizeNoteName('draft...')).toBe('draft')
    expect(sanitizeNoteName('draft  ')).toBe('draft')
    expect(sanitizeNoteName('.hidden')).toBe('hidden')
  })

  it('prefixes names Windows reserves', () => {
    expect(sanitizeNoteName('con')).toBe('_con')
    expect(sanitizeNoteName('NUL')).toBe('_NUL')
    expect(sanitizeNoteName('com1')).toBe('_com1')
    expect(sanitizeNoteName('lpt9.txt')).toBe('_lpt9.txt')
    expect(sanitizeNoteName('console')).toBe('console')
    expect(sanitizeNoteName('com10')).toBe('com10')
  })

  it('keeps / as a folder separator and cleans each part', () => {
    expect(sanitizeNoteName('work/acme: kickoff')).toBe('work/acme - kickoff')
    expect(sanitizeNoteName('work//acme')).toBe('work/acme')
    expect(sanitizeNoteName('con/notes')).toBe('_con/notes')
  })

  it('returns an empty string when nothing is left', () => {
    expect(sanitizeNoteName('...')).toBe('')
    expect(sanitizeNoteName('  ')).toBe('')
  })
})

describe('toNoteFileName', () => {
  it('sanitizes and then adds the extension', () => {
    expect(toNoteFileName('meeting: acme')).toBe('meeting - acme.md')
    expect(toNoteFileName('v1.2 notes')).toBe('v1.2 notes.md')
    expect(toNoteFileName('con')).toBe('_con.md')
  })

  it('does not turn a trailing note extension into part of the name', () => {
    expect(toNoteFileName('todo.txt')).toBe('todo.txt')
    expect(toNoteFileName('  todo.md  ')).toBe('todo.md')
  })

  it('returns an empty string when nothing usable is left', () => {
    expect(toNoteFileName('')).toBe('')
    expect(toNoteFileName('...')).toBe('')
  })

  it('keeps a name made only of forbidden characters usable', () => {
    expect(toNoteFileName('???')).toBe('---.md')
  })
})
