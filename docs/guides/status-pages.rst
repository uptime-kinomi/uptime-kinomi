Status pages
============

kinomi generates status pages from your monitors: plain HTML and CSS, no
JavaScript, readable on a phone and in dark mode. You can have as many
pages as you like, each **public** or **private** (with a password), each
showing a different selection of monitors.

Each page is one file in ``pages/``. The file name is the page's ID and its
address: ``pages/home.toml`` is ``/home/``, and a private ``pages/all.toml``
is ``/private/all/``.

A public page for the family
----------------------------

.. literalinclude:: /examples/catalog/pages/home.toml
   :language: toml
   :caption: pages/home.toml

* ``title``: the heading and the browser tab title.
* ``public = true``: anyone who can reach kinomi can open it.
* ``description``: a line of text under the heading.
* ``[[groups]]``: one block per group of monitors, in this order. A group
  lists monitor IDs in ``monitors``, or selects all monitors with certain
  ``tags``, or both.

Tags keep pages up to date by themselves: give a new monitor the tag
``home``, and it appears in the *At home* group without touching the page.

A page for customers, with badges
---------------------------------

.. literalinclude:: /examples/catalog/pages/web.toml
   :language: toml
   :caption: pages/web.toml

``show_badges = true`` also generates small images for every monitor of the
page, in the style you know from GitHub READMEs (see `Badges`_ below).

A private page with everything
------------------------------

.. literalinclude:: /examples/catalog/pages/all.toml
   :language: toml
   :caption: pages/all.toml

``monitors = ["*"]`` means all monitors. A private page shows more than a
public one:

* the type and target of each monitor (the URL, host and port, ...);
* the last check result and the last error with its time;
* the monitor's ``description``;
* kinomi's own health: version, uptime, length of the check queue, skipped
  checks, average response time, render time and last log write.

Public pages never show any of these.

Private pages need a password file: create users with ``uptime-kinomi
hash-password`` and set ``htpasswd_file`` in ``kinomi.toml``
(:ref:`passwords`).

What a page shows
-----------------

At the top, a banner with the overall state of the page's monitors:

.. list-table::
   :widths: 35 65

   * - ✓ All systems operational
     - No monitor is DOWN.
   * - ⚠ Partial outage
     - Some, but not all monitors are DOWN.
   * - ✗ Major outage
     - Every monitor with data is DOWN.
   * - ⚙ Maintenance
     - No monitor has data outside a maintenance window, and at least one
       is in one.
   * - ? No data yet
     - No monitor has a result yet.

Then ongoing and recent incidents, upcoming maintenance windows, and the
groups. For every monitor:

* its state, as a symbol and a word (never colour alone);
* a heartbeat bar of the last 50 checks (``recent_checks``); hover over a
  bar to see the time and result;
* the uptime over the last 24 hours and the last 30 days;
* the average response time over 24 hours;
* the expiry date of its certificate, for HTTPS monitors.

The pages reload themselves every 60 seconds (``auto_refresh``).

The address of the pages
------------------------

.. list-table::
   :widths: 40 60

   * - ``/``
     - Redirects to ``default_page``, or to the first public page in
       alphabetical order.
   * - ``/home/``
     - The public page ``home``.
   * - ``/private/all/``
     - The private page ``all`` (asks for a password).
   * - ``/home/status.json``
     - The page's data as JSON.
   * - ``/home/feed.atom``
     - The page's news feed.
   * - ``/home/badge/shop.svg``
     - The state badge of the monitor ``shop`` (with ``show_badges``).

Set the start page in ``kinomi.toml``:

.. code-block:: toml

   [pages]
   default_page = "home"

The IDs ``assets``, ``private``, ``push``, ``healthz`` and ``metrics`` are
reserved and cannot be page IDs.

.. _passwords:

Passwords
---------

Private pages use HTTP Basic Auth with an htpasswd file, the same format
Apache and nginx use (bcrypt hashes only). Create it in the configuration
directory:

.. code-block:: sh

   cd ~/.config/uptime-kinomi
   uptime-kinomi hash-password anna >> htpasswd    # prints a random password once
   echo 'chosen password' | uptime-kinomi hash-password ben >> htpasswd
   chmod 600 htpasswd

.. code-block:: toml
   :caption: kinomi.toml

   [server]
   htpasswd_file = "htpasswd"

Every user in the file can open every private page. To remove a user,
delete their line. Restart kinomi after changing the file.

Checking a password takes almost a second on a Pi 1 (that is what makes
bcrypt hard to crack), so kinomi remembers a successful login for 10
minutes. After 5 wrong passwords in a row, it checks at most one password
per second.

.. warning::

   Basic Auth sends the password with every request. Without HTTPS, anyone
   on the network path can read it. In your home network that is usually
   acceptable; on the internet, always use a reverse proxy with HTTPS
   (:doc:`/operations/reverse-proxy`).

Badges
------

With ``show_badges = true``, kinomi generates two SVG images per monitor
of the page:

.. list-table::
   :widths: 50 50

   * - ``/web/badge/shop.svg``
     - ``Web shop | UP``, coloured by state
   * - ``/web/badge/uptime/shop.svg``
     - ``uptime 30d | 99.95 %``: green from 99 %, yellow from 95 %, red
       below

Embed them like any image, e.g. in a README:

.. code-block:: markdown

   ![Shop status](https://status.example.org/web/badge/shop.svg)

Badges of private pages need the password, like the page.

status.json
-----------

Every page has a ``status.json`` with the same information as the page, for
scripts, dashboards or another kinomi:

.. code-block:: sh

   curl -s https://status.example.org/home/status.json

.. code-block:: json

   {
     "schema": 1,
     "kinomi_version": "1.0.0",
     "page": "home",
     "title": "Is the internet down?",
     "generated": "2026-11-01T08:15:00Z",
     "overall": "up",
     "monitors": [
       {
         "id": "internet-dns",
         "name": "Internet (DNS)",
         "group": "Internet",
         "state": "UP",
         "since": "2026-10-30T11:51:57Z",
         "uptime_24h": 100.000,
         "uptime_30d": 99.950,
         "avg_ms_24h": 14,
         "cert_expires": null
       }
     ]
   }

``overall`` is ``up``, ``partial``, ``down``, ``maintenance`` or
``unknown``; ``state`` one of ``UP``, ``WARN``, ``PENDING``, ``DOWN``,
``MAINTENANCE``, ``UNKNOWN``, ``PAUSED``. The private variant adds
``type``, ``target``, ``last_check``, ``last_msg``, ``last_error_time``
and ``last_error`` to each monitor.

A quick check from a shell script:

.. code-block:: sh

   curl -fsS https://status.example.org/home/status.json | grep -q '"overall": "up"' && echo all good

The feed
--------

``feed.atom`` lists the incidents of the page and every time one of its
monitors went DOWN or came back, over the last 30 days. Subscribe to it in
a feed reader, or in a chat that supports feeds, to follow a page without
any notification setup. For links in the feed, set the public address of
kinomi in ``kinomi.toml``:

.. code-block:: toml

   [server]
   base_url = "https://status.example.org"

Your own style
--------------

Put a style sheet into the configuration directory and name it in
``kinomi.toml``. It is added after kinomi's own styles, so your rules win:

.. code-block:: toml
   :caption: kinomi.toml

   [pages]
   custom_css = "custom.css"

.. literalinclude:: /examples/catalog/custom.css
   :language: css
   :caption: custom.css

The pages are static files with no external resources: no web fonts, no
images from other servers, no JavaScript. A strict Content Security Policy
enforces that.

Page keys
---------

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``title``
     - (required)
     - 1-100 characters.
   * - ``public``
     - (required)
     - ``true``: open to everyone; ``false``: needs a password.
   * - ``description``
     - none
     - Text under the title (also on public pages), at most 1000
       characters.
   * - ``show_incidents``
     - ``true``
     - Show incidents (:doc:`incidents-and-maintenance`).
   * - ``show_badges``
     - ``false``
     - Generate badges for the page's monitors.
   * - ``[[groups]]``
     - (at least one)
     - ``name`` (1-100 characters), and ``monitors`` and/or ``tags``.

And in ``kinomi.toml``, section ``[pages]``:

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``timezone``
     - ``"UTC"``
     - Time zone for times on pages and in notifications, e.g.
       ``"Europe/Berlin"``.
   * - ``default_page``
     - first public page
     - Where ``/`` leads.
   * - ``auto_refresh``
     - ``"60s"``
     - How often browsers reload a page; ``"0s"`` switches it off.
   * - ``render_interval``
     - ``"60s"``
     - How often pages are regenerated (when something changed).
   * - ``recent_checks``
     - ``50``
     - Length of the heartbeat bar, 1-200.
   * - ``custom_css``
     - none
     - Your style sheet.
   * - ``output_dir``
     - ``/run/user/<uid>/uptime-kinomi/www`` (in memory) if it exists,
       else ``www`` in the data directory
     - Where the files are written (:doc:`/operations/logs-and-data`).
