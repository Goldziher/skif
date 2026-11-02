import { createRoot } from 'react-dom/client';
import { Button } from '@corp/uikit';
import { Card } from '@corp/uikit/Card';
import { Donut } from 'baseUI/charts/Donut';
import { settings } from 'config';
import { globalName } from 'global-utils';
import { format } from 'helpers/format';
import { clamp } from 'numbers/math';
import { fetchAll } from 'services/api';
import { Home } from 'views/Home';

createRoot(document.body).render(
  <Home
    title={format(settings.title)}
    load={fetchAll}
    chart={<Donut />}
    action={<Button />}
    card={<Card />}
    size={clamp(globalName.length)}
  />,
);
