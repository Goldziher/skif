#!/bin/sh
set -eu
yarn build
npm view web version
