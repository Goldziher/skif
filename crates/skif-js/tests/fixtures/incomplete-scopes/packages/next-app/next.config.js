const plugins = [];

// A function of the phase: nothing in it is known, but Next.js only takes entries from its router.
module.exports = (phase) => plugins.reduce((config, plugin) => plugin(config), { pageExtensions: ['page.tsx'] });
