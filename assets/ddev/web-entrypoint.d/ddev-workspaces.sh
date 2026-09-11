#!/bin/sh
set -eu

# ddev-workspaces: managed asset; do not edit
# DDEV runs this before nginx, so the route is static for the lifetime of the
# shared host. Workspace creation only adds/removes links below this directory.

host=$(printenv DDEV_HOSTNAME 2>/dev/null || true)
if [ -z "$host" ]; then
  project=$(printenv DDEV_PROJECT 2>/dev/null || true)
  if [ -n "$project" ]; then
    host="$project.ddev.site"
  fi
fi
host=$(printf '%s' "$host" | sed 's/[ ,].*//')
if [ -z "$host" ]; then
  exit 0
fi

# Use a bracketed dot instead of a backslash-escaped dot. This keeps the
# replacement literal when the generated regex is passed through macOS sed.
host_regex=$(printf '%s' "$host" | sed 's/[.]/[.]/g')
route_file="/etc/nginx/sites-enabled/ddev-workspaces.conf"
route_tmp="${route_file}.tmp-$$"
trap 'rm -f "$route_tmp"' EXIT HUP INT TERM

cat >"$route_tmp" <<'NGINX'
# ddev-workspaces: generated inside the web container
server {
    listen 80;
    listen 443 ssl;
    server_name ~^(?<ddev_workspace>[a-z0-9][a-z0-9-]*)\.DDEV_WORKSPACES_HOSTNAME$;

    root /var/www/html/.ddev-workspaces/links/$ddev_workspace;
    ssl_certificate /etc/ssl/certs/master.crt;
    ssl_certificate_key /etc/ssl/certs/master.key;
    include /etc/nginx/monitoring.conf;

    index index.php index.htm index.html;
    sendfile off;
    error_log /dev/stdout info;
    access_log /var/log/nginx/access.log;

    location / {
        absolute_redirect off;
        try_files $uri $uri/ /index.php?$query_string;
    }

    location ~ \.php$ {
        try_files $uri =404;
        fastcgi_split_path_info ^(.+\.php)(/.+)$;
        fastcgi_pass unix:/run/php/php-fpm.sock;
        fastcgi_buffers 16 16k;
        fastcgi_buffer_size 32k;
        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
        fastcgi_param SCRIPT_NAME $fastcgi_script_name;
        fastcgi_index index.php;
        include fastcgi_params;
        fastcgi_intercept_errors off;
        fastcgi_read_timeout 10m;
        fastcgi_param SERVER_NAME $host;
        fastcgi_param HTTPS $fcgi_https;
        fastcgi_pass_header "X-Accel-Buffering";
        fastcgi_pass_header "X-Accel-Charset";
        fastcgi_pass_header "X-Accel-Expires";
        fastcgi_pass_header "X-Accel-Limit-Rate";
        fastcgi_pass_header "X-Accel-Redirect";
    }

    location ~* /\.(?!well-known/) {
        deny all;
    }

    location ~* (?:\.(?:bak|conf|dist|fla|in[ci]|log|psd|sh|sql|sw[op])|~)$ {
        deny all;
    }

    include /etc/nginx/common.d/*.conf;
    include /mnt/ddev_config/nginx/*.conf;
}

# Keep malformed or unknown labels under the DDEV wildcard away from the
# parent site's default virtual host. The workspace regex above wins for
# managed labels; this server is the explicit deny path for everything else.
server {
    listen 80;
    listen 443 ssl;
    server_name ~^(?:[^.]+\.)+DDEV_WORKSPACES_HOSTNAME$;
    ssl_certificate /etc/ssl/certs/master.crt;
    ssl_certificate_key /etc/ssl/certs/master.key;
    return 404;
}
NGINX

sed -i "s/DDEV_WORKSPACES_HOSTNAME/$host_regex/" "$route_tmp"
mv -f "$route_tmp" "$route_file"
trap - EXIT HUP INT TERM
