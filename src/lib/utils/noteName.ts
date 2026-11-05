/**
 * Note names typed by the user become file names, and the notes folder is
 * usually synced between macOS, Windows and Linux. These helpers turn what was
 * typed into a name that is valid on all three.
 */

// The backend only indexes files with one of these extensions, and it compares
// them case-sensitively, so `NOTE.MD` is not a note file.
const NOTE_EXTENSIONS = ['.md', '.txt', '.markdown']

// Windows refuses these as a file name, with or without an extension.
const WINDOWS_RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i

// Characters Windows does not allow in a file name, plus `\`, which the backend
// rejects, and control characters.
// eslint-disable-next-line no-control-regex
const ILLEGAL_CHARACTERS = /[<>:"|?*\\\u0000-\u001f]/g

export function hasNoteExtension(name: string): boolean {
  return NOTE_EXTENSIONS.some((extension) => name.endsWith(extension))
}

/** Appends `.md` unless the name already ends in a note extension. */
export function withNoteExtension(name: string): string {
  return hasNoteExtension(name) ? name : `${name}.md`
}

function sanitizeSegment(segment: string): string {
  // Windows drops trailing dots and spaces, so the file on disk would not match
  // the name the app asked for. A leading dot hides the file from the scanner.
  let cleaned = segment
    .replace(ILLEGAL_CHARACTERS, '-')
    .replace(/^[.\s]+/, '')
    .replace(/[.\s]+$/, '')

  if (WINDOWS_RESERVED.test(cleaned.split('.')[0])) {
    cleaned = `_${cleaned}`
  }

  return cleaned
}

/**
 * Makes a typed note name safe as a file name on Windows, macOS and Linux.
 *
 * `/` still separates folders, as before; each part is cleaned on its own.
 * "meeting: acme" becomes "meeting - acme", and any other forbidden character
 * becomes `-`. Returns an empty string when nothing usable is left.
 */
export function sanitizeNoteName(name: string): string {
  return name
    .replace(/:\s+/g, ' - ')
    .split('/')
    .map(sanitizeSegment)
    .filter((segment) => segment.length > 0)
    .join('/')
}

/** The file name to use for a typed note name. */
export function toNoteFileName(name: string): string {
  const sanitized = sanitizeNoteName(name.trim())
  return sanitized ? withNoteExtension(sanitized) : ''
}
