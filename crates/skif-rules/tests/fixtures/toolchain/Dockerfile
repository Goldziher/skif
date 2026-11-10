FROM node:18.19.0-alpine AS build
RUN corepack enable
WORKDIR /app
COPY . .
RUN npm ci && \
    npm run build

FROM node:20-slim
# skif-ignore-next-line foreign-package-manager-command -- the runtime image only reads the registry
RUN npm install -g serve
CMD ["serve", "dist"]
