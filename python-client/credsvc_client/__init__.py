import contextlib
from datetime import datetime
from typing import Union

import requests
from dateutil.parser import parse as dtparse


class Credential:
    user: str
    password: str
    expires_on: datetime

    def __init__(self, user: str, password: str, expires_on: Union[str, datetime]):
        self.user = user
        self.password = password
        if type(expires_on) == str:
            self.expires_on = dtparse(expires_on)
        else:
            self.expires_on = expires_on


class BearerAuth(requests.auth.AuthBase):
    def __init__(self, token):
        self.token = token

    def __call__(self, r):
        r.headers["Authorization"] = "Bearer " + self.token
        return r


class CredentialService:
    url: str
    token: str

    def __init__(self, url: str, token: str):
        self.url = url
        self.token = token

    @contextlib.contextmanager
    def credential(self, service_name: str, timeout_secs: int = 1000):
        response = requests.post(f"{self.url.rstrip('/')}/get", json={
            "service": service_name
        }, timeout=timeout_secs, auth=BearerAuth(self.token))
        response.raise_for_status()

        data = response.json()
        try:
            cred = Credential(data["user"], data["password"], data["expires_on"])
            yield cred
        finally:
            # give back the lease to make it avaliable for others
            response = requests.post(f"{self.url.rstrip('/')}/release", json={
                "lease": data["lease"]
            }, timeout=timeout_secs, auth=BearerAuth(self.token))
            response.raise_for_status()


