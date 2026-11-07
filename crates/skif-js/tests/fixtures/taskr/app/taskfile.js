const notifier = require('node-notifier');

exports.build = function* (task) {
  yield task.clear('dist');
  yield task.source('src/*.js').target('dist');
  notifier.notify('built');
};
