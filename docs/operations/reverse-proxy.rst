HTTPS and a reverse proxy
=========================

kinomi's built-in web server speaks plain HTTP. That is fine in your home
network. To make your status page reachable from the internet, put a web
server in front of it that handles HTTPS: **nginx**, **Apache** or
**Caddy**, with a free certificate from Let's Encrypt. This is called a
*reverse proxy*.

kinomi does not do HTTPS itself on purpose: TLS servers are complex, and
the web servers above do it well and get security updates from your
distribution.

The examples use ``status.example.org``; replace it with your name. Its DNS
record must point to the computer that runs the proxy, and ports 80 and 443
must be reachable from the internet (a port forwarding in your router, if
it's at home).

Two ways to set it up
---------------------

**Mode A** (recommended): the proxy forwards every request to kinomi.
kinomi serves the pages, checks passwords and receives pushes. The proxy
only adds HTTPS.

**Mode B**: the proxy serves the generated page files directly from disk
and checks passwords itself, with kinomi's htpasswd file. Only push requests
(``/push/``) and, if enabled, ``/metrics`` are forwarded to kinomi; without
both, kinomi's server can be switched off. Choose this if you do not want any request from the
internet to reach kinomi's own server.

kinomi's settings for both
--------------------------

kinomi listens only on the computer itself, and accepts the public name:

.. code-block:: toml
   :caption: kinomi.toml

   [server]
   listen = "127.0.0.1:8080"
   hostnames = ["status.example.org"]
   base_url = "https://status.example.org"

``hostnames`` must contain the name in the browser's address bar, because
the proxy passes it on. ``base_url`` is used for links in the Atom feeds.

nginx (mode A)
--------------

.. code-block:: sh

   sudo apt install nginx certbot python3-certbot-nginx

Save this as ``/etc/nginx/sites-available/kinomi``:

.. literalinclude:: /../contrib/nginx/mode-a.conf
   :language: nginx
   :caption: /etc/nginx/sites-available/kinomi

Get the certificate and enable the site:

.. code-block:: sh

   sudo certbot certonly --webroot -w /var/www/html -d status.example.org
   sudo ln -s /etc/nginx/sites-available/kinomi /etc/nginx/sites-enabled/
   sudo nginx -t && sudo systemctl reload nginx

The first ``certbot`` call needs nginx running with the port 80 part of the
file; if nginx refuses to start because the certificate does not exist yet,
comment out the ``server`` block for port 443, get the certificate, and
enable it again. certbot renews the certificate automatically.

Apache (mode A)
---------------

.. code-block:: sh

   sudo apt install apache2 certbot python3-certbot-apache
   sudo a2enmod ssl proxy proxy_http headers

.. literalinclude:: /../contrib/apache/mode-a.conf
   :language: apache
   :caption: /etc/apache2/sites-available/kinomi.conf

.. code-block:: sh

   sudo certbot certonly --apache -d status.example.org
   sudo a2ensite kinomi
   sudo apachectl configtest && sudo systemctl reload apache2

Caddy (mode A)
--------------

Caddy gets and renews certificates by itself and passes the host name on
by default, so the whole configuration is three lines:

.. code-block:: text
   :caption: /etc/caddy/Caddyfile

   status.example.org {
       reverse_proxy 127.0.0.1:8080
   }

.. code-block:: sh

   sudo systemctl reload caddy

Below a path instead of a subdomain
-----------------------------------

All links on kinomi's pages are relative, so the pages also work below a
path like ``https://example.org/status/``. With nginx:

.. code-block:: nginx

   location /status/ {
       proxy_pass http://127.0.0.1:8080/;
       proxy_set_header Host $host;
   }

The push URL is then ``https://example.org/status/push/<token>``.

Mode B
------

In mode B, the web server reads kinomi's page files and its htpasswd file,
so both must be readable by the web server's user. That works best with the
system-wide installation (:ref:`system-wide`): its pages are in
``/run/uptime-kinomi/www``, readable by the group ``kinomi``:

.. code-block:: sh

   sudo usermod -aG kinomi www-data
   sudo systemctl restart nginx

nginx then verifies kinomi's bcrypt passwords itself (Debian 11+, Ubuntu
22.04+ and Raspberry Pi OS bullseye+ support that).

.. literalinclude:: /../contrib/nginx/mode-b.conf
   :language: nginx
   :caption: /etc/nginx/sites-available/kinomi (mode B)

The Apache variant is in ``contrib/apache/mode-b.conf`` in the repository.

Without push monitors and ``/metrics``, switch kinomi's server off:

.. code-block:: toml
   :caption: kinomi.toml

   [server]
   enabled = false

Check it
--------

.. code-block:: sh

   curl -sI https://status.example.org/ | head -n 1

answers ``HTTP/2 302`` (the redirect to your default page). If you get
``421``, the name is missing in ``hostnames``; if you get ``502``, kinomi is
not running or listens on another port.

Health check for monitoring tools
---------------------------------

``/healthz`` answers ``200 ok`` while kinomi works normally, and ``503``
if its internal loops are stuck. Point an external monitor (or a second
kinomi) at ``https://status.example.org/healthz``.

Prometheus
----------

With ``metrics = true`` in ``[server]``, kinomi serves ``/metrics`` in the
Prometheus text format, behind the same password as private pages. It
contains each monitor's state, uptime, response time (in seconds, as
Prometheus expects) and certificate expiry, and kinomi's own health. Give Prometheus its own user:

.. code-block:: sh

   uptime-kinomi hash-password prometheus >> ~/.config/uptime-kinomi/htpasswd

.. code-block:: yaml
   :caption: prometheus.yml

   scrape_configs:
     - job_name: kinomi
       scheme: https
       basic_auth:
         username: prometheus
         password_file: /etc/prometheus/kinomi.password
       static_configs:
         - targets: ["status.example.org"]
