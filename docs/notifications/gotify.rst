Gotify
======

`Gotify <https://gotify.net>`_ is a self-hosted server for push messages,
with an Android app.

#. In the Gotify web interface, open **Apps** and create an application,
   e.g. "kinomi". Copy its token.
#. Store the token in a file only you can read:

   .. code-block:: sh

      echo 'AqTk...' > ~/.config/uptime-kinomi/secrets/gotify.token
      chmod 600 ~/.config/uptime-kinomi/secrets/gotify.token

#. Create the notifier:

.. literalinclude:: /examples/catalog/notifications/gotify.toml
   :language: toml
   :caption: notifications/gotify.toml

Messages have a title (``My blog is DOWN``) and the text. Their priority is
8 for ``down`` and ``reminder`` and 5 otherwise, so the app can treat
outages differently from the rest.

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 25 25 50

   * - Key
     - Default
     - Meaning
   * - ``url``
     - (required)
     - The Gotify server.
   * - ``token`` / ``token_file``
     - (one required)
     - The application token.
   * - ``priority``
     - 8 for down/reminder, 5 otherwise
     - 0 to 10, for all events of this notifier.
