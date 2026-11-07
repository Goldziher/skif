const ncc = require('@vercel/ncc');

module.exports = function (task) {
  task.plugin('ncc', {}, function* (file) {
    file.data = yield ncc(file.base);
  });
};
