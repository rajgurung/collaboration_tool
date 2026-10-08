Hi {{ name }},

{% if count == 1 %}You were mentioned in {{ org }} and haven't seen it yet:{% else %}You have {{ count }} mentions in {{ org }} you haven't seen yet:{% endif %}
{% for item in items %}
{{ item.who }} {{ item.headline }}{% if item.excerpt %}
  "{{ item.excerpt }}"{% endif %}
  {{ item.url }}
{% endfor %}
See all your notifications: {{ notifications_url }}

You get this email when someone tags you with @ and you haven't opened it within 15 minutes.
