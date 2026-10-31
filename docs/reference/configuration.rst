Configuration reference
=======================

This page lists every file type and where its keys are explained. The
complete, normative description is the :doc:`specification`.

``kinomi.toml``
---------------

The global file. Every section and key is optional except
``config_version``. All of them, with their defaults:

.. literalinclude:: /examples/catalog/kinomi.toml
   :language: toml
   :caption: kinomi.toml

.. list-table::
   :header-rows: 1
   :widths: 30 20 50

   * - Key
     - Default
     - Meaning
   * - ``[server] enabled``
     - ``true``
     - The built-in web server (pages, push, ``/healthz``).
   * - ``[server] listen``
     - ``"127.0.0.1:8080"``
     - Address and port. ``"0.0.0.0:8080"`` for the whole network; IPv6 as
       ``"[::]:8080"``.
   * - ``[server] hostnames``
     - ``[]`` (any, with a warning)
     - Names allowed in the ``Host`` header (port ignored). Others get
       ``421``.
   * - ``[server] base_url``
     - none
     - Public address, for links in feeds.
   * - ``[server] htpasswd_file``
     - none
     - Password file for private pages and ``/metrics``.
   * - ``[server] metrics``
     - ``false``
     - Serve Prometheus ``/metrics``.
   * - ``[pages] ...``
     -
     - See :doc:`/guides/status-pages`.
   * - ``[storage] data_dir``
     - ``~/.local/state/uptime-kinomi``
     - State files.
   * - ``[storage] log_dir``
     - ``<data_dir>/log``
     - Check log.
   * - ``[storage] flush_interval``
     - ``"30s"``
     - How often the log is written (1s-10m).
   * - ``[storage] state_interval``
     - ``"10m"``
     - How often state files are written (at least 1m).
   * - ``[storage] log_retention``
     - ``"90d"``
     - Log files older than this are deleted.
   * - ``[storage] history_retention``
     - ``"400d"``
     - Daily totals kept in the state files.
   * - ``[scheduler] workers``
     - ``4``
     - Checks that run at the same time (1-64).
   * - ``[scheduler] max_queue``
     - ``32``
     - Due checks waiting for a worker (1-10000).
   * - ``[heartbeat] url``
     - none
     - URL to call regularly (:doc:`/operations/two-pis`).
   * - ``[heartbeat] interval``
     - ``"60s"``
     - Pause between heartbeats (at least 10s).
   * - ``[defaults] ...``
     -
     - ``interval``, ``timeout``, ``retries``, ``notifications`` for all
       monitors (:ref:`common-keys`).

Other files
-----------

.. list-table::
   :header-rows: 1
   :widths: 30 70

   * - Directory
     - Keys explained in
   * - ``monitors/``
     - :ref:`common-keys`, :doc:`/monitors/http`, :doc:`/monitors/tcp`,
       :doc:`/monitors/dns`, :doc:`/monitors/push`
   * - ``notifications/``
     - :doc:`/notifications/index`, :doc:`/notifications/ntfy`,
       :doc:`/notifications/gotify`, :doc:`/notifications/telegram`,
       :doc:`/notifications/webhook`
   * - ``pages/``
     - :doc:`/guides/status-pages`
   * - ``incidents/``, ``maintenance/``
     - :doc:`/guides/incidents-and-maintenance`

General rules
-------------

* File name = ID: ``^[a-z0-9][a-z0-9_-]{0,63}$``.
* Every file starts with ``config_version = 1``.
* Unknown keys are errors.
* Durations: ``"500ms"``, ``"30s"``, ``"5m"``, ``"2h"``, ``"1d"``; at most
  ``"3650d"``.
* Files are at most 64 KiB, secret files at most 4096 bytes.
* Relative paths are relative to the configuration directory.
* Secrets: ``key = "..."`` inline, or ``key_file = "path"`` (the file's
  content, trimmed).

The configuration directory is the first of: ``--config-dir DIR``,
``$KINOMI_CONFIG_DIR``, ``~/.config/uptime-kinomi`` (if it exists),
``/etc/uptime-kinomi``.

All examples in this documentation are files in the repository
(``docs/examples/``), and the test suite runs ``check-config`` on them, so
they are valid for the version of kinomi the documentation belongs to.
