Webhooks: Discord, Slack, Mattermost, JSON
==========================================

A ``webhook`` notifier sends an HTTP ``POST`` with a JSON body to a URL.
With ``format`` it speaks the dialect of Discord, of Slack (also understood
by Mattermost and Rocket.Chat), or plain JSON for your own programs.

The URL of a chat webhook is a secret: whoever has it can post into your
chat. Keep it in a file with ``url_file``, readable only by you.

Discord
-------

#. In Discord: **Server Settings → Integrations → Webhooks → New Webhook**.
   Choose the channel, then **Copy Webhook URL**.
#. Store it:

   .. code-block:: sh

      echo 'https://discord.com/api/webhooks/...' > ~/.config/uptime-kinomi/secrets/discord.url
      chmod 600 ~/.config/uptime-kinomi/secrets/discord.url

.. literalinclude:: /examples/catalog/notifications/discord.toml
   :language: toml
   :caption: notifications/discord.toml

Slack, Mattermost, Rocket.Chat
------------------------------

Create an *incoming webhook* (Slack: an app with "Incoming Webhooks";
Mattermost: **Integrations → Incoming Webhooks**; Rocket.Chat:
**Administration → Integrations**), store its URL the same way, and use
``format = "slack"``:

.. literalinclude:: /examples/catalog/notifications/mattermost.toml
   :language: toml
   :caption: notifications/mattermost.toml

This one skips ``warn`` messages, so the chat only hears about outages.

Your own program (JSON)
-----------------------

With ``format = "json"`` (the default), kinomi sends all details as
separate fields:

.. code-block:: json

   {
     "event": "down",
     "monitor": "blog",
     "name": "My blog",
     "detail": "HTTP 503",
     "time": "2026-10-22T14:23:11Z",
     "text": "DOWN: My blog (HTTP 503) at 2026-10-22 16:23 CEST"
   }

``event`` is ``down``, ``up``, ``warn`` or ``reminder``; ``monitor`` the
monitor's ID; ``time`` in UTC. ``up`` and ``reminder`` also have
``down_for_s``: how many seconds the monitor has been down.

For example a Home Assistant automation with a *webhook* trigger, which can
turn a lamp red or announce the outage on a speaker:

.. literalinclude:: /examples/catalog/notifications/home-assistant.toml
   :language: toml
   :caption: notifications/home-assistant.toml

In Home Assistant, the fields are available as ``trigger.json.event``,
``trigger.json.name`` and so on.

To see what kinomi sends, point a webhook at a quick listener on another
computer (``nc -l 9000``, then ``url = "http://<that computer>:9000/"``)
and trigger a notification.

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``url`` / ``url_file``
     - (one required)
     - Where to send the ``POST``. Redirects are not followed.
   * - ``format``
     - ``"json"``
     - ``"json"``, ``"discord"`` (sends ``{"content": text}``) or
       ``"slack"`` (sends ``{"text": text}``).

A notification counts as delivered when the server answers with a 2xx
status code.
