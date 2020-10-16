import contextlib
from datetime import datetime, timedelta
from typing import Union
import abc
import sys

import requests
from dateutil.parser import parse as dtparse


class CredentialLease:
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


class AbstractCredentialService(metaclass=abc.ABCMeta):
    @abc.abstractmethod
    def list(self):
        pass

    @abc.abstractmethod
    @contextlib.contextmanager
    def credential_lease(self, service_name: str, **kw):
        yield


class CredentialService(AbstractCredentialService):
    url: str
    token: str
    ssl_verify: bool
    useragent: str

    def __init__(self, url: str, token: str, ssl_verify: bool = True, useragent: str = None):
        self.url = url
        self.token = token
        self.ssl_verify = ssl_verify
        if useragent is None:
            try:
                self.useragent = "/".join(sys.argv[0].split("/")[-2:])
            except:
                self.useragent = "unknown"
        else:
            self.useragent = useragent

    def list(self):
        response = self._request("GET", "/")
        response.raise_for_status()
        return response.json()

    def _request(self, method: str, path: str, **kw):
        headers = kw.get("headers", {})
        headers["User-Agent"] = self.useragent
        kw["headers"] = headers
        return requests.request(
            method,
            f"{self.url.rstrip('/')}{path}",
            auth=BearerAuth(self.token),
            verify=self.ssl_verify,
            **kw
        )

    @contextlib.contextmanager
    def credential_lease(self, service_name: str, timeout_secs: int = 60 * 60 * 10):
        """
        Fetch a credential. In case no free credential is available this method will
        block/wait until one becomes available or timeout_secs is reached

        :param service_name:
        :param timeout_secs: wait at least this many seconds to receive a credential. after that
                a timeout will be raised
        :return:
        """
        response = self._request("POST", "/get", json={
            "service": service_name
        }, timeout=timeout_secs)
        response.raise_for_status()

        data = response.json()
        try:
            cred = CredentialLease(data["user"], data["password"], data["expires_on"])
            yield cred
        finally:
            # give back the lease to make it avaliable for others
            response = self._request("POST", "/release", json={
                "lease": data["lease"]
            })
            response.raise_for_status()


class MockCredentialService(AbstractCredentialService):
    """placeholder to use when no credential service is available or for unittests, ..."""
    user: str
    password: str

    def __init__(self, user: str, password: str):
        self.user = user
        self.password = password

    def list(self):
        return dict()

    def credential_lease(self, service_name: str, **kw):
        yield CredentialLease(self.user, self.password, expires_on=datetime.now() + timedelta(minutes=10))

