import { Button } from '@/components/Button';
import { clamp } from '@/lib/math';

import logo from './assets/logo.svg';

export default function App() {
  return (
    <main>
      <img src={logo} alt="logo" />
      <Button label={`value ${clamp(5, 0, 3)}`} />
    </main>
  );
}
