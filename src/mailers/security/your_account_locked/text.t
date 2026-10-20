Hi {{ name }},

Someone typed a wrong app password 5 times while trying to see logins in {{ org }}, so your account is locked and signed out everywhere.{% if claude_disconnected %} Claude was disconnected.{% endif %}

To unlock it, reset your password:

{{ forgot_url }}

If it wasn't you, someone may be using your phone or computer.
