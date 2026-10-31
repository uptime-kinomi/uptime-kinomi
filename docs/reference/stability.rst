Compatibility and updates
=========================

kinomi updates itself every week (:doc:`/operations/updates`), so an update
must never break a working setup. From version 1.0 on, kinomi follows
`semantic versioning <https://semver.org/>`_: within a major version
(1.x), updates only add things. Before 1.0, configuration details can still
change between versions; the :doc:`changelog` says what to adapt.

What stays the same within 1.x
------------------------------

* **Your configuration.** Every key keeps its name, meaning and default. A
  configuration that ``check-config`` accepts is accepted by every later
  1.x version. The only exception is a security fix that has to reject a
  dangerous value; the changelog lists it under "Security".
* **The command line**: commands, options and exit codes, so scripts keep
  working.
* **Addresses**: the page URLs, ``status.json``, ``feed.atom``, badges,
  ``/push/<token>``, ``/healthz`` and ``/metrics``. Push URLs in your
  scripts and badges in your README keep working.
* **Machine-readable output**: ``status.json`` (``"schema": 1``) and the
  ``/metrics`` names only get new fields, never lose or rename one.
* **Your data**: every version reads the logs and state files of all
  earlier versions (from 0.2.0 on), so the history survives updates. A test
  checks this with files written by the real old releases.

What may change in any version
------------------------------

* The look of the pages (HTML and CSS). If you use ``custom.css``, check it
  after an update.
* The wording of messages: check results, notifications, the feed, error
  messages and the output of ``check-config``.

Breaking changes
----------------

A change that would break a configuration only comes with a new major
version (2.0). Before that, ``check-config`` warns about what is going to
change for at least one 1.x version, so you can adapt in time.

The weekly update **never installs a new major version** on its own. It
writes into its log that one is available:

.. code-block:: text

   2.0.0 is available, a new major version (1.4.2 stays installed).

Read the changelog, adapt your configuration, then install it yourself:

.. code-block:: sh

   uptime-kinomi self-update --release 2.0.0
   uptime-kinomi check-config
   systemctl --user restart uptime-kinomi

The complete rules are in the :doc:`specification <specification>`, §16.
