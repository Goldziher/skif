import { format } from '../../lib/format';

// A route only under the unknown `pageExtensions`: every file of `app/` is treated as an entry.
export default function Settings() {
  return format('settings');
}
