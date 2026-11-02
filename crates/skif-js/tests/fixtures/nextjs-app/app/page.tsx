import Link from 'next/link';

import Counter from '@/components/Counter';
import { getUser } from '@/lib/db';

export default async function Home() {
  const user = await getUser();
  return (
    <main>
      <p>{user.name}</p>
      <Counter />
      <Link href="/about">About</Link>
    </main>
  );
}
