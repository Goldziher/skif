const path = require('path');
const { HtmlRspackPlugin } = require('@rspack/core');

module.exports = {
  entry: { main: path.resolve(__dirname, 'src', 'main.jsx') },
  resolve: {
    modules: [path.resolve(__dirname, 'lib'), 'node_modules'],
  },
  plugins: [new HtmlRspackPlugin({ template: './public/index.html' })],
};
